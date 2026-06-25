//! mcp-server-plugin-manager — standalone MCP server for plugin management.
//! Communicates via stdio JSON-RPC (MCP protocol).
//!
//! Tool: plugin_manager
//! Parameters:
//!   action: "list" | "install" | "uninstall" | "enable" | "disable" | "config"
//!   name: string (required for all except list)
//!   url: string (required for install)
//!   config: object (required for config action)

use anyhow::{Context, Result};
use mcp_server_util::*;
use omniagent::plugin;
use serde_json::Value;
use sqlx::PgPool;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Environment helpers
// ---------------------------------------------------------------------------

/// Read DATA_DIR from env with a default fallback.
fn data_dir() -> String {
    std::env::var("DATA_DIR")
        .or_else(|_| std::env::var("HOME").map(|h| format!("{}/.omniagent", h)))
        .unwrap_or_else(|_| "/opt/data".to_string())
}

// ---------------------------------------------------------------------------
// Tool: plugin_manager — list
// ---------------------------------------------------------------------------

fn handle_list(pool: &PgPool, _args: &Value) -> Result<(String, bool)> {
    let handle = tokio::runtime::Handle::current();
    let rows = handle.block_on(async {
        plugin::list_plugins(pool)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to list plugins: {}", e))
    })?;

    let details: Vec<plugin::PluginDetail> =
        rows.iter().map(|r| plugin::enrich_plugin(r)).collect();

    let output = serde_json::to_string_pretty(&details)?;
    Ok((output, false))
}

// ---------------------------------------------------------------------------
// Tool: plugin_manager — install
// ---------------------------------------------------------------------------

fn handle_install(pool: &PgPool, args: &Value) -> Result<(String, bool)> {
    let url = args["url"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing required argument for install: 'url'"))?;

    let dir = data_dir();
    let manifest = plugin::installer::install_from_url(url, &dir)
        .map_err(|e| anyhow::anyhow!("Installation failed: {}", e))?;

    let manifest_json = serde_json::to_value(&manifest)?;
    let plugin_type_str = match manifest.plugin_type {
        plugin::PluginType::Platform => "platform",
        plugin::PluginType::Mcp => "mcp",
        plugin::PluginType::Provider => "provider",
    };

    let handle = tokio::runtime::Handle::current();
    let row = handle.block_on(async {
        plugin::upsert_plugin(
            pool,
            plugin::UpsertPluginParams {
                name: &manifest.name,
                plugin_type: plugin_type_str,
                version: &manifest.version,
                source: Some(url),
                manifest: &manifest_json,
                config: &serde_json::json!({}),
            },
        )
        .await
        .map_err(|e| anyhow::anyhow!("Failed to register plugin in DB: {}", e))
    })?;

    let detail = plugin::enrich_plugin(&row);
    let output = serde_json::to_string_pretty(&detail)?;

    Ok((
        format!(
            "Plugin '{}' version {} installed successfully.\n\n{}",
            manifest.name, manifest.version, output
        ),
        false,
    ))
}

// ---------------------------------------------------------------------------
// Tool: plugin_manager — uninstall
// ---------------------------------------------------------------------------

fn handle_uninstall(pool: &PgPool, args: &Value) -> Result<(String, bool)> {
    let name = args["name"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing required argument for uninstall: 'name'"))?;

    // Remove from database
    let handle = tokio::runtime::Handle::current();
    let deleted = handle.block_on(async {
        plugin::delete_plugin(pool, name)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to delete plugin from DB: {}", e))
    })?;

    // Remove from disk
    let dir = data_dir();
    let disk_result = plugin::installer::uninstall(name, &dir);

    let mut parts = vec![];
    if deleted {
        parts.push("Removed from registry".to_string());
    } else {
        parts.push("Not found in registry".to_string());
    }
    match disk_result {
        Ok(_) => parts.push("Removed from disk".to_string()),
        Err(e) => parts.push(format!("Disk removal note: {}", e)),
    }

    Ok((format!("Plugin '{}': {}", name, parts.join("; ")), false))
}

// ---------------------------------------------------------------------------
// Tool: plugin_manager — enable
// ---------------------------------------------------------------------------

fn handle_enable(pool: &PgPool, args: &Value) -> Result<(String, bool)> {
    let name = args["name"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing required argument for enable: 'name'"))?;

    let handle = tokio::runtime::Handle::current();
    let row = handle.block_on(async {
        plugin::update_plugin_status(pool, name, "enabled")
            .await
            .map_err(|e| anyhow::anyhow!("Failed to enable plugin: {}", e))
    })?;

    Ok((
        format!(
            "Plugin '{}' enabled (current status: {})",
            name, row.status
        ),
        false,
    ))
}

// ---------------------------------------------------------------------------
// Tool: plugin_manager — disable
// ---------------------------------------------------------------------------

fn handle_disable(pool: &PgPool, args: &Value) -> Result<(String, bool)> {
    let name = args["name"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing required argument for disable: 'name'"))?;

    let handle = tokio::runtime::Handle::current();
    let row = handle.block_on(async {
        plugin::update_plugin_status(pool, name, "disabled")
            .await
            .map_err(|e| anyhow::anyhow!("Failed to disable plugin: {}", e))
    })?;

    Ok((
        format!(
            "Plugin '{}' disabled (current status: {})",
            name, row.status
        ),
        false,
    ))
}

// ---------------------------------------------------------------------------
// Tool: plugin_manager — config
// ---------------------------------------------------------------------------

fn handle_config(pool: &PgPool, args: &Value) -> Result<(String, bool)> {
    let name = args["name"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing required argument for config: 'name'"))?;

    let config = args
        .get("config")
        .ok_or_else(|| anyhow::anyhow!("Missing required argument for config: 'config'"))?;

    let handle = tokio::runtime::Handle::current();
    let row = handle.block_on(async {
        plugin::update_plugin_config(pool, name, config)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to update plugin config: {}", e))
    })?;

    let detail = plugin::enrich_plugin(&row);
    let output = serde_json::to_string_pretty(&detail)?;

    Ok((
        format!("Plugin '{}' config updated.\n\n{}", name, output),
        false,
    ))
}

// ---------------------------------------------------------------------------
// Main dispatch
// ---------------------------------------------------------------------------

fn handle_plugin_manager(pool: &PgPool, args: &Value) -> Result<(String, bool)> {
    let action = args["action"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing required argument: 'action'"))?;

    match action {
        "list" => handle_list(pool, args),
        "install" => handle_install(pool, args),
        "uninstall" => handle_uninstall(pool, args),
        "enable" => handle_enable(pool, args),
        "disable" => handle_disable(pool, args),
        "config" => handle_config(pool, args),
        _ => anyhow::bail!(
            "Unknown action '{}'. Valid actions: list, install, uninstall, enable, disable, config",
            action
        ),
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    let database_url =
        std::env::var("DATABASE_URL").context("DATABASE_URL must be set")?;
    let pool = omniagent::db::connect(&database_url)
        .await
        .context("Failed to connect to database")?;
    let pool = Arc::new(pool);

    let p = pool.clone();
    let handler: ToolHandler = Box::new(move |args: &Value| handle_plugin_manager(&p, args));

    let tools = vec![McpToolEntry {
        def: McpToolDef {
            name: "plugin_manager".to_string(),
            description:
                "Manage plugins: list, install, uninstall, enable, disable, or update config. \
                 Use action='list' to see all plugins. \
                 Use action='install' with a url to install from a tarball/zip. \
                 Use action='uninstall' with a name to remove a plugin. \
                 Use action='enable' or 'disable' with a name to toggle plugin status. \
                 Use action='config' with a name and config object to update plugin settings."
                    .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "description": "Action to perform: list, install, uninstall, enable, disable, config",
                        "enum": ["list", "install", "uninstall", "enable", "disable", "config"]
                    },
                    "name": {
                        "type": "string",
                        "description": "Plugin name (required for all actions except list)"
                    },
                    "url": {
                        "type": "string",
                        "description": "Download URL for install action (.tar.gz, .tgz, or .zip)"
                    },
                    "config": {
                        "type": "object",
                        "description": "Configuration object for config action"
                    }
                },
                "required": ["action"]
            }),
        },
        handler,
    }];

    let server_info = ServerInfo {
        name: "mcp-server-plugin-manager".to_string(),
        version: "0.1.0".to_string(),
    };

    run_server(server_info, tools).await
}
