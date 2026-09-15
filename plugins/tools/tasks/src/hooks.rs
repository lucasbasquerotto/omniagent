#![allow(dead_code, unused_imports)]
//! Hook tools for the unified `tasks` MCP server.
//!
//! Full parity with the core hooks HTTP API (src/server/hooks.rs):
//!
//! - `GET    /hooks`              -> list_hooks (query: event, enabled)
//! - `GET    /hooks/{id}`         -> get_hook
//! - `POST   /hooks`              -> create_hook
//! - `PATCH  /hooks/{id}`         -> update_hook (tri-state fields)
//! - `PATCH  /hooks/{id}/toggle`  -> toggle_hook
//! - `DELETE /hooks/{id}`         -> delete_hook
//! - `POST   /hooks/{id}/fire`    -> fire_hook
//! - `GET    /hooks/{id}/threads` -> list_hook_threads
//!
//! Like the kanban tools, the plugin only talks HTTP to the core server: it
//! never writes tasks.yml itself. API errors are surfaced verbatim (the core
//! error text is passed through unchanged).

use anyhow::{anyhow, Result};
use mcp_server_util::*;
use serde_json::{json, Value};
use sqlx::PgPool;
use std::sync::{Arc, OnceLock};
use tokio::sync::RwLock;

// ---------------------------------------------------------------------------
// HTTP helpers (same shape as the kanban tools)
// ---------------------------------------------------------------------------

/// Core server base URL (configure message `base_url`; default localhost:8080).
static BASE_URL: OnceLock<String> = OnceLock::new();

/// Set the core API base URL (from the plugin configure message).
pub fn set_base_url(url: String) {
    let _ = BASE_URL.set(url);
}

fn api_url(path: &str) -> String {
    let base = BASE_URL
        .get()
        .map(String::as_str)
        .unwrap_or("http://localhost:8080");
    format!("{base}{path}")
}

async fn api_call(method: reqwest::Method, path: &str, body: Option<&Value>) -> Result<Value> {
    let client = reqwest::Client::new();
    let mut req = client.request(method, api_url(path));
    if let Some(b) = body {
        req = req.json(b);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| anyhow!("Hooks API error: {e}"))?;
    let status = resp.status();
    let json: Value = resp
        .json()
        .await
        .map_err(|e| anyhow!("Invalid hooks API response: {e}"))?;
    if !status.is_success() || json.get("success").and_then(|s| s.as_bool()) == Some(false) {
        let msg = json["error"].as_str().unwrap_or("operation failed");
        anyhow::bail!("Hooks API error: {msg}");
    }
    Ok(json)
}

/// Percent-encode a query-string value (RFC 3986 unreserved set kept).
fn encode_query_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Attach every PRESENT (non-null) key of `keys` from `args` to `body`.
/// For updates the caller passes explicit JSON null through instead (tri-state).
fn copy_present(args: &Value, body: &mut serde_json::Map<String, Value>, keys: &[&str]) {
    for k in keys {
        if let Some(v) = args.get(*k) {
            if !v.is_null() {
                body.insert((*k).to_string(), v.clone());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /hooks (optional event / enabled filters).
async fn handle_list(args: &Value) -> Result<(String, bool)> {
    let mut query: Vec<String> = Vec::new();
    if let Some(event) = args.get("event").and_then(|v| v.as_str()) {
        if !event.is_empty() {
            query.push(format!("event={}", encode_query_value(event)));
        }
    }
    if let Some(enabled) = args.get("enabled").and_then(|v| v.as_bool()) {
        query.push(format!("enabled={enabled}"));
    }
    let path = if query.is_empty() {
        "/hooks".to_string()
    } else {
        format!("/hooks?{}", query.join("&"))
    };
    let resp = api_call(reqwest::Method::GET, &path, None).await?;
    let data = resp.get("data").cloned().unwrap_or(resp);
    Ok((data.to_string(), false))
}

/// GET /hooks/{id}
async fn handle_get(args: &Value) -> Result<(String, bool)> {
    let id = args["id"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing required argument: 'id'"))?;
    let resp = api_call(
        reqwest::Method::GET,
        &format!("/hooks/{}", encode_query_value(id)),
        None,
    )
    .await?;
    let data = resp.get("data").cloned().unwrap_or(resp);
    Ok((data.to_string(), false))
}

/// POST /hooks
async fn handle_create(args: &Value) -> Result<(String, bool)> {
    let name = args["name"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing required argument: 'name'"))?;
    let event = args["event"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing required argument: 'event'"))?;
    let mut body = serde_json::Map::new();
    body.insert("name".to_string(), json!(name));
    body.insert("event".to_string(), json!(event));
    copy_present(
        args,
        &mut body,
        &[
            "scope",
            "target",
            "count",
            "mode",
            "prompt",
            "action_id",
            "profile",
            "channel",
            "plan",
            "template",
            "toolset",
            "enabled",
        ],
    );
    let resp = api_call(reqwest::Method::POST, "/hooks", Some(&Value::Object(body))).await?;
    Ok((resp.to_string(), false))
}

/// PATCH /hooks/{id} - tri-state: omit a key to keep it, pass null or an empty
/// string to clear it, pass a value to set it (same semantics as the API).
async fn handle_update(args: &Value) -> Result<(String, bool)> {
    let id = args["id"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing required argument: 'id'"))?;
    let keys = [
        "name",
        "event",
        "scope",
        "target",
        "count",
        "mode",
        "prompt",
        "action_id",
        "profile",
        "channel",
        "plan",
        "template",
        "toolset",
        "enabled",
    ];
    let mut body = serde_json::Map::new();
    for k in keys {
        if let Some(v) = args.get(k) {
            body.insert(k.to_string(), v.clone());
        }
    }
    if body.is_empty() {
        return Ok((
            "No updatable field provided (name, event, scope, target, count, mode, \
             prompt, action_id, profile, channel, plan, template, toolset, enabled)"
                .to_string(),
            true,
        ));
    }
    let resp = api_call(
        reqwest::Method::PATCH,
        &format!("/hooks/{}", encode_query_value(id)),
        Some(&Value::Object(body)),
    )
    .await?;
    Ok((resp.to_string(), false))
}

/// PATCH /hooks/{id}/toggle (no body; flips `enabled`).
async fn handle_toggle(args: &Value) -> Result<(String, bool)> {
    let id = args["id"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing required argument: 'id'"))?;
    let resp = api_call(
        reqwest::Method::PATCH,
        &format!("/hooks/{}/toggle", encode_query_value(id)),
        None,
    )
    .await?;
    Ok((resp.to_string(), false))
}

/// DELETE /hooks/{id}
async fn handle_delete(args: &Value) -> Result<(String, bool)> {
    let id = args["id"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing required argument: 'id'"))?;
    let resp = api_call(
        reqwest::Method::DELETE,
        &format!("/hooks/{}", encode_query_value(id)),
        None,
    )
    .await?;
    Ok((format!("Hook '{id}' deleted ({resp})"), false))
}

/// POST /hooks/{id}/fire (manual trigger: no counter increment, no reset).
async fn handle_fire(args: &Value) -> Result<(String, bool)> {
    let id = args["id"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing required argument: 'id'"))?;
    let resp = api_call(
        reqwest::Method::POST,
        &format!("/hooks/{}/fire", encode_query_value(id)),
        None,
    )
    .await?;
    Ok((resp.to_string(), false))
}

/// GET /hooks/{id}/threads
async fn handle_threads(args: &Value) -> Result<(String, bool)> {
    let id = args["id"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing required argument: 'id'"))?;
    let resp = api_call(
        reqwest::Method::GET,
        &format!("/hooks/{}/threads", encode_query_value(id)),
        None,
    )
    .await?;
    let data = resp.get("data").cloned().unwrap_or(resp);
    Ok((data.to_string(), false))
}

// ---------------------------------------------------------------------------
// Tool registration
// ---------------------------------------------------------------------------

pub fn build_tools(_pool: &Arc<RwLock<Option<PgPool>>>) -> Vec<McpToolEntry> {
    let list_handler: ToolHandler = Box::new(move |args: Value, _meta: Option<McpMeta>| {
        Box::pin(async move { handle_list(&args).await })
    });
    let get_handler: ToolHandler = Box::new(move |args: Value, _meta: Option<McpMeta>| {
        Box::pin(async move { handle_get(&args).await })
    });
    let create_handler: ToolHandler = Box::new(move |args: Value, _meta: Option<McpMeta>| {
        Box::pin(async move { handle_create(&args).await })
    });
    let update_handler: ToolHandler = Box::new(move |args: Value, _meta: Option<McpMeta>| {
        Box::pin(async move { handle_update(&args).await })
    });
    let toggle_handler: ToolHandler = Box::new(move |args: Value, _meta: Option<McpMeta>| {
        Box::pin(async move { handle_toggle(&args).await })
    });
    let delete_handler: ToolHandler = Box::new(move |args: Value, _meta: Option<McpMeta>| {
        Box::pin(async move { handle_delete(&args).await })
    });
    let fire_handler: ToolHandler = Box::new(move |args: Value, _meta: Option<McpMeta>| {
        Box::pin(async move { handle_fire(&args).await })
    });
    let threads_handler: ToolHandler = Box::new(move |args: Value, _meta: Option<McpMeta>| {
        Box::pin(async move { handle_threads(&args).await })
    });

    vec![
        McpToolEntry {
            def: McpToolDef {
                name: "list_hooks".to_string(),
                description: "List hooks from tasks.yml (same source of truth and same shape as GET /hooks)."
                    .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "event": { "type": "string", "description": "Optional event filter (maps to GET /hooks?event=)" },
                        "enabled": { "type": "boolean", "description": "Optional enabled filter (maps to GET /hooks?enabled=)" }
                    },
                    "required": []
                }),
            },
            handler: list_handler,
        },
        McpToolEntry {
            def: McpToolDef {
                name: "get_hook".to_string(),
                description: "Get one hook by id (maps to GET /hooks/{id}).".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": { "id": { "type": "string", "description": "Hook id (its name in tasks.yml)" } },
                    "required": ["id"]
                }),
            },
            handler: get_handler,
        },
        McpToolEntry {
            def: McpToolDef {
                name: "create_hook".to_string(),
                description: "Create a hook (full parity with POST /hooks): an event -> action binding stored in tasks.yml. 'event' is the trigger event and 'mode' selects agentic prompt execution or an action id.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Unique hook name; the name IS the hook id (rejected if it already exists)" },
                        "event": { "type": "string", "description": "Event that triggers the hook (e.g. thread_completed, message_received)" },
                        "scope": { "type": "string", "description": "Optional scope (all|channel|profile|thread|task)" },
                        "target": { "type": "string", "description": "Optional scope target (channel/profile id etc.)" },
                        "count": { "type": "integer", "description": "Optional fire counter / interval" },
                        "mode": { "type": "string", "description": "'agentic' (default) or 'action'" },
                        "prompt": { "type": "string", "description": "Prompt for agentic mode" },
                        "action_id": { "type": "string", "description": "Action id for action mode" },
                        "profile": { "type": "string", "description": "Optional profile for the spawned thread" },
                        "channel": { "type": "string", "description": "Optional channel for the spawned thread" },
                        "plan": { "type": "boolean", "description": "Plan mode for the spawned thread" },
                        "template": { "type": "string", "description": "Optional template for the spawned thread" },
                        "toolset": { "type": "string", "description": "Optional toolset for the spawned thread" },
                        "enabled": { "type": "boolean", "description": "Whether the hook is active (default true)" }
                    },
                    "required": ["name", "event"]
                }),
            },
            handler: create_handler,
        },
        McpToolEntry {
            def: McpToolDef {
                name: "update_hook".to_string(),
                description: "Update a hook (full parity with PATCH /hooks/{id}). Fields are tri-state: omit to keep, pass null or an empty string to clear, pass a value to set.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Hook id to update" },
                        "name": { "type": "string", "description": "New name; the name IS the id, so this re-keys the hook (same as the API)" },
                        "event": { "type": "string", "description": "New trigger event" },
                        "scope": { "type": "string", "description": "New scope; null clears it" },
                        "target": { "type": "string", "description": "New scope target; null clears it" },
                        "count": { "type": "integer", "description": "New counter/interval; null clears it" },
                        "mode": { "type": "string", "description": "'agentic' or 'action'" },
                        "prompt": { "type": "string", "description": "New prompt; null clears it" },
                        "action_id": { "type": "string", "description": "Action id; null clears it" },
                        "profile": { "type": "string", "description": "New profile; null clears it" },
                        "channel": { "type": "string", "description": "New channel; null clears it" },
                        "plan": { "type": "boolean", "description": "Plan mode; null clears it" },
                        "template": { "type": "string", "description": "New template; null clears it" },
                        "toolset": { "type": "string", "description": "New toolset; null clears it" },
                        "enabled": { "type": "boolean", "description": "Enable/disable the hook" }
                    },
                    "required": ["id"]
                }),
            },
            handler: update_handler,
        },
        McpToolEntry {
            def: McpToolDef {
                name: "toggle_hook".to_string(),
                description: "Flip a hook's enabled flag (maps to PATCH /hooks/{id}/toggle, no body).".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": { "id": { "type": "string", "description": "Hook id to toggle" } },
                    "required": ["id"]
                }),
            },
            handler: toggle_handler,
        },
        McpToolEntry {
            def: McpToolDef {
                name: "delete_hook".to_string(),
                description: "Delete a hook by id (maps to DELETE /hooks/{id}).".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": { "id": { "type": "string", "description": "Hook id to delete" } },
                    "required": ["id"]
                }),
            },
            handler: delete_handler,
        },
        McpToolEntry {
            def: McpToolDef {
                name: "fire_hook".to_string(),
                description: "Manually trigger a hook (maps to POST /hooks/{id}/fire; no counter increment, no reset). Returns the spawned thread id.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": { "id": { "type": "string", "description": "Hook id to fire" } },
                    "required": ["id"]
                }),
            },
            handler: fire_handler,
        },
        McpToolEntry {
            def: McpToolDef {
                name: "list_hook_threads".to_string(),
                description: "List the threads a hook spawned (maps to GET /hooks/{id}/threads).".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": { "id": { "type": "string", "description": "Hook id" } },
                    "required": ["id"]
                }),
            },
            handler: threads_handler,
        },
    ]
}
