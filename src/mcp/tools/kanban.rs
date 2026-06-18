use crate::mcp::{truncate_content, AppContext, McpTool, McpToolResult, DEFAULT_MAX_TOOL_OUTPUT_CHARS};
use anyhow::Result;
use serde_json::Value;
use sql_forge::sql_forge;
use sqlx::FromRow;
use std::sync::Arc;
use chrono::{DateTime, Utc};

#[derive(Debug, FromRow)]
struct KanbanTaskRow {
    id: String,
    title: String,
    body: Option<String>,
    status: String,
    priority: Option<i32>,
    assignee: Option<String>,
    created_at: Option<DateTime<Utc>>,
    updated_at: Option<DateTime<Utc>>,
}

pub fn create_kanban_task_tool() -> McpTool {
    McpTool {
        name: "create_kanban_task".to_string(),
        description: "Create a new kanban task. Adds a task to the kanban board with optional body, status, priority, and assignee.".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "Task title"
                },
                "body": {
                    "type": "string",
                    "description": "Optional task description/body"
                },
                "status": {
                    "type": "string",
                    "description": "Optional status (default: 'backlog'). One of: backlog, todo, ready, running, review, done, blocked",
                    "enum": ["backlog", "todo", "ready", "running", "review", "done", "blocked"]
                },
                "priority": {
                    "type": "integer",
                    "description": "Optional priority (default: 0). 0=Low, 1=Med, 3=High, 5=Critical"
                },
                "assignee": {
                    "type": "string",
                    "description": "Optional assignee name"
                }
            },
            "required": ["title"]
        }),
        handler: Arc::new(|args: Value, ctx: AppContext| -> Result<McpToolResult> {
            let title = args["title"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing required argument: 'title'"))?;

            if title.is_empty() {
                anyhow::bail!("Task title must not be empty");
            }

            let body = args["body"].as_str().unwrap_or("");
            let status = args["status"].as_str().unwrap_or("backlog");
            let priority = args["priority"].as_i64().unwrap_or(0) as i32;
            let assignee = args["assignee"].as_str().unwrap_or("");

            // Validate status
            let valid_statuses = ["backlog", "todo", "ready", "running", "review", "done", "blocked"];
            if !valid_statuses.contains(&status) {
                anyhow::bail!("Invalid status '{}'. Must be one of: backlog, todo, ready, running, review, done, blocked", status);
            }

            let pool = ctx.pool.clone();

            tokio::task::block_in_place(|| {
                let handle = tokio::runtime::Handle::current();
                handle.block_on(async {
                    // Generate a unique ID using a database sequence idiom
                    let id = format!("task_{:x}", {
                        use std::time::{SystemTime, UNIX_EPOCH};
                        SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_nanos()
                    });

                    sql_forge!(
                        r#"
                        INSERT INTO kanban_tasks (id, title, body, status, priority, assignee)
                        VALUES (:id, :title, :body, :status, :priority, :assignee)
                        "#,
                        ( :id = &id, :title = title, :body = body, :status = status, :priority = priority, :assignee = assignee )
                    )
                    .execute(&pool)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to create kanban task: {}", e))?;

                    Ok::<_, anyhow::Error>(McpToolResult {
                        call_id: String::new(),
                        content: format!("Kanban task '{}' created with id '{}' and status '{}'", title, id, status),
                        is_error: false,
                    })
                })
            })
        }),
    }
}

pub fn list_kanban_tasks_tool() -> McpTool {
    McpTool {
        name: "list_kanban_tasks".to_string(),
        description: "List all kanban tasks, optionally filtered by status. Returns tasks grouped by status column.".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "status": {
                    "type": "string",
                    "description": "Optional status filter. One of: backlog, todo, ready, running, review, done, blocked"
                }
            }
        }),
        handler: Arc::new(|args: Value, ctx: AppContext| -> Result<McpToolResult> {
            let status_filter = args["status"].as_str().map(|s| s.to_string());
            let pool = ctx.pool.clone();

            let result: Vec<KanbanTaskRow> = tokio::task::block_in_place(|| {
                let handle = tokio::runtime::Handle::current();
                handle.block_on(async {
                    if let Some(ref status) = status_filter {
                        sql_forge!(
                            KanbanTaskRow,
                            r#"
                            SELECT id, title, body, status, priority, assignee, created_at, updated_at
                            FROM kanban_tasks
                            WHERE status = :status
                            ORDER BY priority DESC, created_at DESC
                            "#,
                            ( :status = status )
                        )
                        .fetch_all(&pool)
                        .await
                        .map_err(|e| anyhow::anyhow!("Failed to list kanban tasks: {}", e))
                    } else {
                        sql_forge!(
                            KanbanTaskRow,
                            r#"
                            SELECT id, title, body, status, priority, assignee, created_at, updated_at
                            FROM kanban_tasks
                            WHERE 1 = :_one
                            ORDER BY status, priority DESC, created_at DESC
                            "#,
                            ( :_one = 1i32 )
                        )
                        .fetch_all(&pool)
                        .await
                        .map_err(|e| anyhow::anyhow!("Failed to list kanban tasks: {}", e))
                    }
                })
            })?;

            use std::collections::BTreeMap;
            let mut grouped: BTreeMap<String, Vec<serde_json::Value>> = BTreeMap::new();

            for r in &result {
                let entry = serde_json::json!({
                    "id": r.id,
                    "title": r.title,
                    "body": r.body,
                    "status": r.status,
                    "priority": r.priority.unwrap_or(0),
                    "assignee": r.assignee,
                    "created_at": r.created_at.map(|t| t.to_rfc3339()),
                    "updated_at": r.updated_at.map(|t| t.to_rfc3339()),
                });
                grouped.entry(r.status.clone()).or_default().push(entry);
            }

            let output = serde_json::to_string_pretty(&grouped)?;
            Ok(McpToolResult {
                call_id: String::new(),
                content: truncate_content(&output, DEFAULT_MAX_TOOL_OUTPUT_CHARS),
                is_error: false,
            })
        }),
    }
}

pub fn update_kanban_task_tool() -> McpTool {
    McpTool {
        name: "update_kanban_task".to_string(),
        description: "Update a kanban task's fields (title, body, status, priority, assignee). Only provided fields are updated.".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "Task ID to update"
                },
                "title": {
                    "type": "string",
                    "description": "New title"
                },
                "body": {
                    "type": "string",
                    "description": "New body/description"
                },
                "status": {
                    "type": "string",
                    "description": "New status. One of: backlog, todo, ready, running, review, done, blocked"
                },
                "priority": {
                    "type": "integer",
                    "description": "New priority. 0=Low, 1=Med, 3=High, 5=Critical"
                },
                "assignee": {
                    "type": "string",
                    "description": "New assignee"
                }
            },
            "required": ["id"]
        }),
        handler: Arc::new(|args: Value, ctx: AppContext| -> Result<McpToolResult> {
            let id = args["id"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing required argument: 'id'"))?;

            let pool = ctx.pool.clone();
            let id_clone = id.to_string();

            tokio::task::block_in_place(|| {
                let handle = tokio::runtime::Handle::current();
                handle.block_on(async {
                    // Check task exists
                    let exists: bool = sql_forge!(
                        scalar i64,
                        "SELECT COUNT(*) FROM kanban_tasks WHERE id = :id",
                        ( :id = &id_clone )
                    )
                    .fetch_one(&pool)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to check task existence: {e}"))?
                    > 0;

                    if !exists {
                        anyhow::bail!("Kanban task '{id_clone}' not found");
                    }

                    // Apply individual UPDATEs per provided field (static SQL per field)
                    if let Some(title) = args["title"].as_str() {
                        if title.is_empty() {
                            anyhow::bail!("Task title must not be empty");
                        }
                        sql_forge!(
                            "UPDATE kanban_tasks SET title = :val, updated_at = NOW() WHERE id = :id",
                            ( :val = title, :id = &id_clone )
                        )
                        .execute(&pool)
                        .await
                        .map_err(|e| anyhow::anyhow!("Failed to update title: {e}"))?;
                    }
                    if args.get("body").is_some() {
                        let body = args["body"].as_str().unwrap_or("");
                        sql_forge!(
                            "UPDATE kanban_tasks SET body = :val, updated_at = NOW() WHERE id = :id",
                            ( :val = body, :id = &id_clone )
                        )
                        .execute(&pool)
                        .await
                        .map_err(|e| anyhow::anyhow!("Failed to update body: {e}"))?;
                    }
                    if let Some(status) = args["status"].as_str() {
                        let valid_statuses = ["backlog", "todo", "ready", "running", "review", "done", "blocked"];
                        if !valid_statuses.contains(&status) {
                            anyhow::bail!("Invalid status '{status}'");
                        }
                        sql_forge!(
                            "UPDATE kanban_tasks SET status = :val, updated_at = NOW() WHERE id = :id",
                            ( :val = status, :id = &id_clone )
                        )
                        .execute(&pool)
                        .await
                        .map_err(|e| anyhow::anyhow!("Failed to update status: {e}"))?;
                    }
                    if args.get("priority").is_some() {
                        let priority = args["priority"].as_i64().unwrap_or(0) as i32;
                        sql_forge!(
                            "UPDATE kanban_tasks SET priority = :val, updated_at = NOW() WHERE id = :id",
                            ( :val = priority, :id = &id_clone )
                        )
                        .execute(&pool)
                        .await
                        .map_err(|e| anyhow::anyhow!("Failed to update priority: {e}"))?;
                    }
                    if args.get("assignee").is_some() {
                        let assignee = args["assignee"].as_str().unwrap_or("");
                        sql_forge!(
                            "UPDATE kanban_tasks SET assignee = :val, updated_at = NOW() WHERE id = :id",
                            ( :val = assignee, :id = &id_clone )
                        )
                        .execute(&pool)
                        .await
                        .map_err(|e| anyhow::anyhow!("Failed to update assignee: {e}"))?;
                    }

                    Ok::<_, anyhow::Error>(())
                })
            })?;

            Ok(McpToolResult {
                call_id: String::new(),
                content: format!("Kanban task '{}' updated successfully", id),
                is_error: false,
            })
        }),
    }
}

pub fn delete_kanban_task_tool() -> McpTool {
    McpTool {
        name: "delete_kanban_task".to_string(),
        description: "Delete a kanban task by its ID.".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "Task ID to delete"
                }
            },
            "required": ["id"]
        }),
        handler: Arc::new(|args: Value, ctx: AppContext| -> Result<McpToolResult> {
            let id = args["id"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing required argument: 'id'"))?;

            let pool = ctx.pool.clone();
            let id_clone = id.to_string();

            let deleted = tokio::task::block_in_place(|| {
                let handle = tokio::runtime::Handle::current();
                handle.block_on(async {
                    sql_forge!(
                        "DELETE FROM kanban_tasks WHERE id = :id",
                        ( :id = &id_clone )
                    )
                    .execute(&pool)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to delete kanban task: {}", e))
                })
            })?;

            if deleted.rows_affected() == 0 {
                anyhow::bail!("Kanban task '{}' not found", id);
            }

            Ok(McpToolResult {
                call_id: String::new(),
                content: format!("Kanban task '{}' deleted successfully", id),
                is_error: false,
            })
        }),
    }
}
