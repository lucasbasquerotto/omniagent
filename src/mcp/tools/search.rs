use crate::mcp::{truncate_content, AppContext, McpTool, McpToolResult, DEFAULT_MAX_TOOL_OUTPUT_CHARS};
use anyhow::Result;
use serde_json::Value;
use sql_forge::sql_forge;
use sqlx::FromRow;
use std::sync::Arc;

#[derive(Debug, FromRow)]
struct SearchResult {
    id: i64,
    role: String,
    content: String,
}

pub fn search_messages_tool(ctx: &AppContext) -> McpTool {
    let pool = ctx.pool.clone();
    McpTool {
        name: "search_messages".to_string(),
        description: "SEARCH CONVERSATION HISTORY in the database by text content. Use this to find past messages and discussions. This is a DATABASE SEARCH of conversation text, NOT a file reader. It searches message content, not files on disk. For reading files use filesystem_read.".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Text to search for in message content"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results (default: 10, max: 50)",
                    "default": 10
                },
                "channel_id": {
                    "type": "integer",
                    "description": "Optional channel ID to restrict search to"
                }
            },
            "required": ["query"]
        }),
        handler: Arc::new(move |args: Value, _ctx: AppContext| -> Result<McpToolResult> {
            let query = args["query"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing 'query' argument"))?;
            let limit = args["limit"]
                .as_i64()
                .unwrap_or(10)
                .min(50);
            let channel_id = args["channel_id"].as_i64();

            let pool = pool.clone();
            let query_owned = query.to_string();

            let results: Vec<SearchResult> = tokio::task::block_in_place(|| {
                let handle = tokio::runtime::Handle::current();
                handle.block_on(async {
                    if let Some(cid) = channel_id {
                        sql_forge!(
                            SearchResult,
                            r#"
                            SELECT m.id, m.role, m.content FROM messages m
                            JOIN threads t ON t.id = m.thread_id
                            WHERE t.channel_id = :channel_id
                              AND m.content ILIKE '%' || :query || '%'
                            ORDER BY m.created_at DESC
                            LIMIT :limit
                            "#,
                            ( :channel_id = cid, :query = &query_owned, :limit = limit )
                        )
                        .fetch_all(&pool)
                        .await
                    } else {
                        sql_forge!(
                            SearchResult,
                            r#"
                            SELECT id, role, content FROM messages
                            WHERE content ILIKE '%' || :query || '%'
                            ORDER BY created_at DESC
                            LIMIT :limit
                            "#,
                            ( :query = &query_owned, :limit = limit )
                        )
                        .fetch_all(&pool)
                        .await
                    }
                })
            })
            .map_err(|e: sqlx::Error| anyhow::anyhow!(e))?;

            if results.is_empty() {
                return Ok(McpToolResult {
                    call_id: String::new(),
                    content: "No matching messages found.".to_string(),
                    is_error: false,
                });
            }

            let mut lines = Vec::new();
            for r in &results {
                let preview = if r.content.len() > 200 {
                    let truncate_to = r.content
                        .char_indices()
                        .nth(200)
                        .map(|(i, _)| i)
                        .unwrap_or(r.content.len());
                    format!("{}...", &r.content[..truncate_to])
                } else {
                    r.content.clone()
                };
                lines.push(format!("#{} [{}]: {}", r.id, r.role, preview));
            }

            let output = format!("Found {} result(s):\n{}", results.len(), lines.join("\n\n"));
            Ok(McpToolResult {
                call_id: String::new(),
                content: truncate_content(&output, DEFAULT_MAX_TOOL_OUTPUT_CHARS),
                is_error: false,
            })
        }),
    }
}

pub fn search_wiki_tool(ctx: &AppContext) -> McpTool {
    let _ = ctx; // keep for future use (Qdrant integration)
    McpTool {
        name: "search_wiki".to_string(),
        description: "SEARCH WIKI DOCUMENTATION by text content in local wiki/markdown files. Use this to find relevant documentation in wiki files. Searches inside .md files under the profile's wiki directory. For reading specific wiki files, use filesystem_read.".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Text to search for in wiki files"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results (default: 10, max: 30)",
                    "default": 10
                },
                "profile": {
                    "type": "string",
                    "description": "Profile name whose wiki to search (default: 'default')"
                }
            },
            "required": ["query"]
        }),
        handler: Arc::new(move |args: Value, ctx: AppContext| -> Result<McpToolResult> {
            let query = args["query"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing 'query' argument"))?;
            let limit = args["limit"]
                .as_i64()
                .unwrap_or(10)
                .min(30) as usize;
            let profile = args["profile"]
                .as_str()
                .unwrap_or("default");

            let wiki_dir = format!("{}/profiles/{}/wiki", ctx.data_dir, profile);
            let wiki_path = std::path::Path::new(&wiki_dir);

            if !wiki_path.exists() {
                return Ok(McpToolResult {
                    call_id: String::new(),
                    content: format!("Wiki directory not found for profile '{}': {}", profile, wiki_dir),
                    is_error: false,
                });
            }

            let query_lower = query.to_lowercase();
            let mut results: Vec<String> = Vec::new();

            let entries = walkdir::WalkDir::new(wiki_path)
                .max_depth(5)
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().is_file());

            for entry in entries {
                    let path = entry.path().to_path_buf();
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        let content_lower = content.to_lowercase();
                        if content_lower.contains(&query_lower) {
                            let matching_lines: Vec<&str> = content
                                .lines()
                                .filter(|line| line.to_lowercase().contains(&query_lower))
                                .take(3)
                                .collect();
                            let preview = if matching_lines.is_empty() {
                                content.lines().next().unwrap_or("").to_string()
                            } else {
                                matching_lines.join(" | ")
                            };
                            let rel_path = path.strip_prefix(wiki_path)
                                .map(|p| p.to_string_lossy().to_string())
                                .unwrap_or_default();
                            results.push(format!("{}: {}", rel_path, preview));
                        }
                    }
                    if results.len() >= limit {
                        break;
                    }
            }

            results.sort();
            results.truncate(limit);

            if results.is_empty() {
                return Ok(McpToolResult {
                    call_id: String::new(),
                    content: "No matching wiki content found.".to_string(),
                    is_error: false,
                });
            }

            let output = format!("Found {} wiki result(s):\n{}", results.len(), results.join("\n\n"));
            Ok(McpToolResult {
                call_id: String::new(),
                content: truncate_content(&output, DEFAULT_MAX_TOOL_OUTPUT_CHARS),
                is_error: false,
            })
        }),
    }
}
