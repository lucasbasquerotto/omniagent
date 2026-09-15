//! mcp-server-tasks: the unified built-in MCP tool plugin for tasks.
//!
//! Superset of the former `kanban` and `cron` tool plugins plus the hooks
//! operations. The three families keep their historical tool (operation)
//! names; only the plugin prefix changes (`kanban__x` / `cron__x` -> `tasks__x`):
//!
//! - kanban: create_kanban_task, list_kanban_tasks, update_kanban_task,
//!   delete_kanban_task, add_kanban_dependency, remove_kanban_dependency,
//!   kanban_review_task
//! - cron:   create_cron_job, list_cron_jobs, update_cron_job, delete_cron_job
//! - hooks:  list_hooks, get_hook, create_hook, update_hook, toggle_hook,
//!   delete_hook, fire_hook, list_hook_threads
//!
//! Why one plugin: the three families share the same config (database_url +
//! core base_url), the same two sources of truth (the kanban DB and
//! config/tasks.yml) and the same lifecycle. One binary avoids a third
//! overlapping surface and keeps the tool/API parity drift in a single place.

mod cron;
mod hooks;
mod kanban;

use anyhow::Result;
use mcp_server_util::*;
use serde_json::Value;
use sqlx::PgPool;
use std::sync::Arc;
use tokio::sync::RwLock;

fn data_dir() -> String {
    std::env::var("OMNI_DIR").unwrap_or_else(|_| "/opt/omni".to_string())
}

/// Plugin config - received via the configure message.
#[derive(Debug, Clone)]
struct PluginConfig {
    pub database_url: String,
    pub base_url: String,
}

impl PluginConfig {
    fn from_json(v: &Value) -> Self {
        Self {
            database_url: v
                .get("database_url")
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_else(|| {
                    eprintln!("FATAL: database_url not in configure message");
                    std::process::exit(1);
                }),
            base_url: v
                .get("base_url")
                .and_then(|v| v.as_str())
                .unwrap_or("http://localhost:8080")
                .to_string(),
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Channels live in {OMNI_DIR}/config/channels.yml - set the global data dir.
    omniagent::channels_yaml::set_data_dir(&data_dir());

    // Shared pool - populated by the configure callback before any tool call.
    let pool: Arc<RwLock<Option<PgPool>>> = Arc::new(RwLock::new(None));

    let mut tools = kanban::build_tools(&pool);
    tools.extend(cron::build_tools(&pool));
    tools.extend(hooks::build_tools(&pool));

    let server_info = ServerInfo {
        name: "mcp-server-tasks".to_string(),
        version: "0.1.0".to_string(),
    };

    run_server_with_config(server_info, tools, {
        let p = pool.clone();
        Some(move |params: Value| {
            let config = PluginConfig::from_json(&params);
            kanban::set_base_url(config.base_url.clone());
            hooks::set_base_url(config.base_url.clone());
            tokio::task::block_in_place(|| {
                let rt = tokio::runtime::Handle::current();
                let new_pool = rt
                    .block_on(omniagent::db::connect(&config.database_url))
                    .expect("Failed to connect to database");
                *p.blocking_write() = Some(new_pool);
            });
            tracing::info!("tasks plugin configured (database_url + base_url)");
        })
    })
    .await
}
