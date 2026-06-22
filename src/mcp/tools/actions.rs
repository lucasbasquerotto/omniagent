//! Built-in "actions" tool — triggers system actions like kanban dispatcher,
//! relevance indexer, and hindsight populator.

use anyhow::Result;
use serde_json::Value;
use std::sync::Arc;

use crate::mcp::{AppContext, McpTool, McpToolResult};

/// Returns the list of built-in action tools.
pub fn tools() -> Vec<McpTool> {
    vec![
        kanban_dispatcher_tool(),
        relevance_indexer_tool(),
        hindsight_populator_tool(),
    ]
}

fn kanban_dispatcher_tool() -> McpTool {
    McpTool {
        name: "actions_kanban_dispatcher".to_string(),
        description: "Trigger the kanban dispatcher — picks up pending kanban tasks and creates agent threads for them.".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {},
            "required": []
        }),
        handler: Arc::new(|_args: Value, ctx: AppContext| -> Result<McpToolResult> {
            let pool = ctx.pool.clone();
            let data_dir = ctx.data_dir.clone();
            tokio::spawn(async move {
                if let Err(e) = crate::scheduler::run_kanban_dispatcher(&pool, &data_dir).await {
                    tracing::error!("[actions] kanban_dispatcher failed: {:?}", e);
                }
            });
            Ok(McpToolResult {
                call_id: "".to_string(),
                content: "Kanban dispatcher triggered".to_string(),
                is_error: false,
            })
        }),
    }
}

fn relevance_indexer_tool() -> McpTool {
    McpTool {
        name: "actions_relevance_indexer".to_string(),
        description: "Trigger the relevance indexer — scans wiki files and updates the relevant-index.md based on recency and reference count.".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {},
            "required": []
        }),
        handler: Arc::new(|_args: Value, ctx: AppContext| -> Result<McpToolResult> {
            let pool = ctx.pool.clone();
            let data_dir = ctx.data_dir.clone();
            tokio::spawn(async move {
                if let Err(e) = crate::relevance::run_relevance_indexer(&pool, &data_dir).await {
                    tracing::error!("[actions] relevance_indexer failed: {:?}", e);
                }
            });
            Ok(McpToolResult {
                call_id: "".to_string(),
                content: "Relevance indexer triggered".to_string(),
                is_error: false,
            })
        }),
    }
}

fn hindsight_populator_tool() -> McpTool {
    McpTool {
        name: "actions_hindsight_populator".to_string(),
        description: "Trigger the hindsight populator — queries recent messages from the database and retains them into the omniagent-hindsight persistent memory store for future recall.".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {},
            "required": []
        }),
        handler: Arc::new(|_args: Value, ctx: AppContext| -> Result<McpToolResult> {
            let pool = ctx.pool.clone();
            let data_dir = ctx.data_dir.clone();
            tokio::spawn(async move {
                if let Err(e) = crate::hindsight_populator::run_hindsight_populator(&pool, &data_dir).await {
                    tracing::error!("[actions] hindsight_populator failed: {:?}", e);
                }
            });
            Ok(McpToolResult {
                call_id: "".to_string(),
                content: "Hindsight populator triggered".to_string(),
                is_error: false,
            })
        }),
    }
}
