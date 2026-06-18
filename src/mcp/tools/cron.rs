use crate::mcp::{truncate_content, AppContext, McpTool, McpToolResult, DEFAULT_MAX_TOOL_OUTPUT_CHARS};
use anyhow::Result;
use serde_json::Value;
use sql_forge::sql_forge;
use sqlx::FromRow;
use std::sync::Arc;
use chrono::{DateTime, Utc};

#[derive(Debug, FromRow)]
struct CronJobListRow {
    id: String,
    name: Option<String>,
    schedule: String,
    prompt: Option<String>,
    enabled: Option<bool>,
    last_run_at: Option<DateTime<Utc>>,
    next_run_at: Option<DateTime<Utc>>,
    created_at: Option<DateTime<Utc>>,
}

pub fn create_cron_job_tool() -> McpTool {
    McpTool {
        name: "create_cron_job".to_string(),
        description: "Create a new cron job. Schedules a recurring task with a cron expression and a prompt to execute. Provide a unique short name (lowercase, underscores, no spaces) as 'name', and optionally a human-readable 'display_name'.".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "A unique short name for this cron job (lowercase, underscores, no spaces). Example: 'hourly-message-count'"
                },
                "display_name": {
                    "type": "string",
                    "description": "Optional human-readable display name. Example: 'Hourly Message Count'. If omitted, the name is used as display_name."
                },
                "schedule": {
                    "type": "string",
                    "description": "Cron schedule expression in 7-field quartz format (sec min hour day month weekday year). Examples: '0 0 9 * * 1-5 *' for weekdays at 9am, '0 0 * * * * *' every hour"
                },
                "prompt": {
                    "type": "string",
                    "description": "The prompt/message to execute when the cron job triggers"
                },
                "skills": {
                    "type": "string",
                    "description": "Optional comma-separated list of skill names to enable for this job"
                }
            },
            "required": ["name", "schedule", "prompt"]
        }),
        handler: Arc::new(|args: Value, ctx: AppContext| -> Result<McpToolResult> {
            let name = args["name"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing required argument: 'name'"))?;

            let schedule = args["schedule"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing required argument: 'schedule'"))?;

            let prompt = args["prompt"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing required argument: 'prompt'"))?;

            let display_name = args["display_name"].as_str().unwrap_or(name);
            let skills_str = args["skills"].as_str().unwrap_or("");
            let display_name_owned = display_name.to_string();
            let name_owned = name.to_string();

            if name.is_empty() {
                anyhow::bail!("Job name must not be empty");
            }
            if schedule.is_empty() {
                anyhow::bail!("Schedule must not be empty");
            }
            if prompt.is_empty() {
                anyhow::bail!("Prompt must not be empty");
            }

            // Generate a unique ID
            let id = format!("cron_{:x}", {
                use std::time::{SystemTime, UNIX_EPOCH};
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            });

            // Parse skills as JSON array
            let skills_json: Value = if skills_str.is_empty() {
                serde_json::json!([])
            } else {
                let parts: Vec<String> = skills_str
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                serde_json::json!(parts)
            };

            let pool = ctx.pool.clone();

            tokio::task::block_in_place(|| {
                let handle = tokio::runtime::Handle::current();
                handle.block_on(async {
                    sql_forge!(
                        r#"
                        INSERT INTO cron_jobs (id, name, display_name, schedule, prompt, skills)
                        VALUES (:id, :name, :display_name, :schedule, :prompt, :skills)
                        "#,
                        ( :id = &id, :name = &name_owned, :display_name = &display_name_owned, :schedule = schedule, :prompt = prompt, :skills = skills_json.to_string() )
                    )
                    .execute(&pool)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to create cron job: {}", e))?;

                    Ok::<_, anyhow::Error>(())
                })
            })?;

            Ok(McpToolResult {
                call_id: String::new(),
                content: format!(
                    "Cron job '{}' created with id '{}', schedule '{}'",
                    name, id, schedule
                ),
                is_error: false,
            })
        }),
    }
}

pub fn list_cron_jobs_tool() -> McpTool {
    McpTool {
        name: "list_cron_jobs".to_string(),
        description: "List all cron jobs with their schedule, status, and last/next run times."
            .to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {}
        }),
        handler: Arc::new(|_: Value, ctx: AppContext| -> Result<McpToolResult> {
            let pool = ctx.pool.clone();

            let rows: Vec<CronJobListRow> = tokio::task::block_in_place(|| {
                let handle = tokio::runtime::Handle::current();
                handle.block_on(async {
                    sql_forge!(
                        CronJobListRow,
                        r#"
                        SELECT id, name, schedule, prompt, enabled, last_run_at, next_run_at, created_at
                        FROM cron_jobs
                        WHERE 1 = :_one
                        ORDER BY created_at DESC
                        "#,
                        ( :_one = 1i32 )
                    )
                    .fetch_all(&pool)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to list cron jobs: {}", e))
                })
            })?;

            let jobs: Vec<serde_json::Value> = rows
                .into_iter()
                .map(|r| {
                    serde_json::json!({
                        "id": r.id,
                        "name": r.name,
                        "schedule": r.schedule,
                        "prompt_preview": r.prompt.as_ref().map(|p| if p.len() > 100 {
                            format!("{}...", &p[..100])
                        } else {
                            p.clone()
                        }),
                        "enabled": r.enabled.unwrap_or(false),
                        "last_run_at": r.last_run_at,
                        "next_run_at": r.next_run_at,
                        "created_at": r.created_at.map(|t| t.to_rfc3339()),
                    })
                })
                .collect();

            let output = serde_json::to_string_pretty(&serde_json::json!({ "jobs": jobs }))?;
            Ok(McpToolResult {
                call_id: String::new(),
                content: truncate_content(&output, DEFAULT_MAX_TOOL_OUTPUT_CHARS),
                is_error: false,
            })
        }),
    }
}

pub fn delete_cron_job_tool() -> McpTool {
    McpTool {
        name: "delete_cron_job".to_string(),
        description: "Delete a cron job by its name. The 'name' parameter is the short unique job name (not the id or display name)."
            .to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The short unique name of the cron job to delete (e.g., 'hourly-message-count')"
                }
            },
            "required": ["name"]
        }),
        handler: Arc::new(|args: Value, ctx: AppContext| -> Result<McpToolResult> {
            let name = args["name"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing required argument: 'name'"))?;

            let pool = ctx.pool.clone();
            let name_owned = name.to_string();

            let deleted = tokio::task::block_in_place(|| {
                let handle = tokio::runtime::Handle::current();
                handle.block_on(async {
                    sql_forge!(
                        r#"
                        DELETE FROM cron_jobs WHERE name = :name
                        "#,
                        ( :name = &name_owned )
                    )
                    .execute(&pool)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to delete cron job: {}", e))
                })
            })?;

            if deleted.rows_affected() == 0 {
                anyhow::bail!("Cron job '{}' not found", name);
            }

            Ok(McpToolResult {
                call_id: String::new(),
                content: format!("Cron job '{}' deleted successfully", name),
                is_error: false,
            })
        }),
    }
}
