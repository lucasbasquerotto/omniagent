//! mcp-server-cron: standalone MCP server for cron job management.
//! Communicates via stdio JSON-RPC (MCP protocol).
//!
//! Tools: create_cron_job, list_cron_jobs, delete_cron_job, update_cron_job
//!
//! PARITY (2026-09-15, kanban-tool-parity task): every field the schedule HTTP
//! API accepts (POST /schedule, PATCH /schedule/{id}, GET /schedule?active=) is
//! expressible through these tools; see the wiki note
//! `Reference/Omniagent/Tool-Api-Parity-Audit.md`.
//!
//! Definitions live in {OMNI_DIR}/config/tasks.yml (`schedules:` key) - the
//! git-tracked source of truth. Runtime state (cadence) is tracked implicitly
//! via the threads each schedule creates (threads.schedule_task_id) and the
//! task_runs bookkeeping table.

use anyhow::Result;
use mcp_server_util::*;
use omniagent::db;
use omniagent::tasks_yaml::{self, ScheduleDef};
use serde_json::Value;
use sqlx::PgPool;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::RwLock;

/// OMNI_DIR (data_dir) - config files live in {data_dir}/config/.
fn data_dir() -> String {
    std::env::var("OMNI_DIR").unwrap_or_else(|_| "/opt/omni".to_string())
}

/// Resolve a channel id to its NAME - with string ids the id IS the name
/// (channels.yml key). Verified to exist in the yml; unknown -> None.
async fn channel_name_for_id(_pool: &PgPool, id: &str) -> Option<String> {
    omniagent::channels_yaml::exists(id).then(|| id.to_string())
}

fn validate_5field(schedule: &str) -> Result<()> {
    let fields: Vec<&str> = schedule.split_whitespace().collect();
    if fields.len() != 5 {
        return Err(anyhow::anyhow!(
            "Invalid cron expression '{}': expected 5 fields (min hour day month weekday), got {} fields",
            schedule,
            fields.len()
        ));
    }
    let cron_expr = format!("0 {}", schedule);
    cron::Schedule::from_str(&cron_expr)
        .map_err(|e| anyhow::anyhow!("Invalid cron expression '{}': {}", schedule, e))?;
    Ok(())
}

/// Slugify a job name into its yml key. The name IS the key (there is no
/// separate id), so create and the rename path of update must derive the key
/// exactly the same way. Mirrors the API's `generate_id`.
fn generate_id(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Argument helpers (parity with the schedule HTTP API)
// ---------------------------------------------------------------------------

/// Tri-state string argument, matching `PATCH /schedule/{id}`: an ABSENT key
/// leaves the stored value unchanged (`None`), an explicit JSON `null` or a
/// blank string CLEARS it (`Some(None)`), any other string SETS it
/// (`Some(Some(v))`). A plain `Option<&str>` cannot tell `null` from absent, so
/// clears used to be silently dropped.
fn tri_state_str(args: &Value, key: &str) -> Option<Option<String>> {
    match args.get(key) {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(s)) => {
            if s.trim().is_empty() {
                Some(None)
            } else {
                Some(Some(s.clone()))
            }
        }
        Some(_) => None,
    }
}

/// Tri-state bool argument (same semantics as [`tri_state_str`]).
fn tri_state_bool(args: &Value, key: &str) -> Option<Option<bool>> {
    match args.get(key) {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::Bool(b)) => Some(Some(*b)),
        Some(_) => None,
    }
}

// ---------------------------------------------------------------------------
// Tool: create_cron_job
// ---------------------------------------------------------------------------

async fn handle_create(
    pool: &PgPool,
    args: &Value,
    meta: Option<&McpMeta>,
) -> Result<(String, bool)> {
    let name = args["name"].as_str().unwrap_or("");
    let schedule = args["schedule"].as_str().unwrap_or("");
    let prompt = args["prompt"].as_str();
    let skills_str = args["skills"].as_str().unwrap_or("");
    // channel: explicit channel_id wins; else the CURRENT channel from the
    // agent's runtime context (_meta.channel_id); else no channel (default).
    // channel: explicit `channel` (the API field name) wins, then the legacy
    // `channel_id` alias, then the CURRENT channel from the agent's runtime
    // context (_meta.channel_id); else no channel (the default applies).
    let channel_id_arg = args["channel"]
        .as_str()
        .or_else(|| args["channel_id"].as_str())
        .map(|s| s.to_string())
        .or_else(|| meta.and_then(|m| m.channel_id.clone()));
    // profile: explicit argument wins; else the agent's ACTIVE profile from
    // _meta.profile_name (the job runs under that profile when fired).
    let profile_owned = args["profile"]
        .as_str()
        .map(|s| s.to_string())
        .or_else(|| meta.and_then(|m| m.profile_name.clone()));
    let profile_arg = profile_owned.as_deref();
    let mode = args["mode"].as_str().unwrap_or("agentic");
    let action_id = args["action_id"].as_str();
    let silent = args["silent"].as_bool();
    // Full parity with POST /schedule (CreateScheduleRequest): every field the
    // schedule API accepts is expressible here. This tool writes the SAME
    // tasks.yml source of truth directly (no HTTP hop), so a parameter the
    // tool does not read would be silently dropped from the job definition.
    let template = args["template"].as_str();
    let toolset = args["toolset"].as_str();
    let plan = args["plan"].as_bool();
    // `active` (API wording) / `enabled` (yml wording); default true.
    let enabled = args["active"]
        .as_bool()
        .or_else(|| args["enabled"].as_bool())
        .unwrap_or(true);

    if name.is_empty() {
        return Err(anyhow::anyhow!("Job name must not be empty"));
    }
    if schedule.is_empty() {
        return Err(anyhow::anyhow!("Schedule must not be empty"));
    }
    validate_5field(schedule)?;
    if mode == "agentic" && prompt.unwrap_or("").is_empty() {
        return Err(anyhow::anyhow!("Prompt must not be empty for agentic mode"));
    }
    if mode == "action" && action_id.unwrap_or("").is_empty() {
        return Err(anyhow::anyhow!("action_id is required for action mode"));
    }
    if mode != "agentic" && mode != "action" {
        return Err(anyhow::anyhow!(
            "Invalid mode '{}'. Must be 'agentic' or 'action'",
            mode
        ));
    }

    // The name IS the key: the job is identified by its (slugified) name.
    let id: String = name
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();

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

    // yml stores channel NAME - resolve from id (if given), else default (None).
    let channel_name = match channel_id_arg {
        Some(cid) => channel_name_for_id(pool, &cid).await,
        None => None,
    };

    let mut tasks = tasks_yaml::load_tasks_or_empty(&data_dir());
    if tasks.schedules.contains_key(&id) {
        return Err(anyhow::anyhow!("Cron job '{}' already exists", name));
    }

    let def = ScheduleDef {
        enabled,
        channel: channel_name,
        profile: profile_arg.map(|s| s.to_string()),
        plan,
        cron: schedule.to_string(),
        prompt: Some(prompt.unwrap_or("").to_string()),
        action: if mode == "action" {
            action_id.map(|s| s.to_string())
        } else {
            None
        },
        template: template.map(|s| s.to_string()),
        skills: Some(skills_json.to_string()),
        silent: Some(silent.unwrap_or(false)),
        toolset: toolset.map(|s| s.to_string()),
    };
    tasks.schedules.insert(id.clone(), def);
    tasks_yaml::save_tasks(&data_dir(), &tasks)
        .map_err(|e| anyhow::anyhow!("Failed to save tasks.yml: {}", e))?;

    Ok((
        format!("✅ Created cron job **{}** (`{}`)", name, id),
        false,
    ))
}

// ---------------------------------------------------------------------------
// Tool: list_cron_jobs
// ---------------------------------------------------------------------------

async fn handle_list(_pool: &PgPool, args: &Value) -> Result<(String, bool)> {
    // `active` / `enabled` filter - parity with GET /schedule?active=.
    let active_filter = args["active"]
        .as_bool()
        .or_else(|| args["enabled"].as_bool());
    let tasks = tasks_yaml::load_tasks_or_empty(&data_dir());

    let rows: Vec<(&String, &ScheduleDef)> = tasks
        .schedules
        .iter()
        .filter(|(_, row)| active_filter.map(|a| row.enabled == a).unwrap_or(true))
        .collect();

    if rows.is_empty() {
        return Ok(("_No cron jobs match._".to_string(), false));
    }

    let mut lines = vec!["**Cron Jobs:**".to_string()];
    for (i, (id, row)) in rows.iter().enumerate() {
        let status = if row.enabled { "🟢" } else { "🔴" };
        let mode_display = if row.action.is_some() {
            "action"
        } else {
            "agentic"
        };
        // Surface the definition fields the API exposes (template/toolset/
        // plan/action/silent), so the tool output describes the whole job.
        let mut extra: Vec<String> = Vec::new();
        if let Some(a) = row.action.as_deref() {
            extra.push(format!("action_id={}", a));
        }
        if let Some(t) = row.toolset.as_deref() {
            extra.push(format!("toolset={}", t));
        }
        if let Some(t) = row.template.as_deref() {
            extra.push(format!("template={}", t));
        }
        if let Some(p) = row.plan {
            extra.push(format!("plan={}", p));
        }
        if row.silent == Some(true) {
            extra.push("silent".to_string());
        }
        let extra = if extra.is_empty() {
            String::new()
        } else {
            format!(" | {}", extra.join(" "))
        };
        let prompt_preview = row
            .prompt
            .as_deref()
            .unwrap_or("")
            .chars()
            .take(80)
            .collect::<String>();
        let channel = row.channel.clone().unwrap_or_else(|| "default".to_string());
        lines.push(format!(
            "{}. {} **{}**\n   - Schedule: `{}` | Mode: {} | Channel: {}{}\n   - Runs are visible via threads (schedule_task_id = `{}`)\n   - Prompt: {}",
            i + 1, status, id, row.cron, mode_display, channel, extra, id, prompt_preview
        ));
    }

    Ok((lines.join("\n"), false))
}

// ---------------------------------------------------------------------------
// Tool: delete_cron_job
// ---------------------------------------------------------------------------

async fn handle_delete(_pool: &PgPool, args: &Value) -> Result<(String, bool)> {
    let job_id = args["job_id"].as_str().unwrap_or("");
    if job_id.is_empty() {
        return Err(anyhow::anyhow!("Missing required argument: 'job_id'"));
    }

    let mut tasks = tasks_yaml::load_tasks_or_empty(&data_dir());
    if tasks.schedules.remove(job_id).is_none() {
        return Err(anyhow::anyhow!("Cron job `{}` not found", job_id));
    }
    tasks_yaml::save_tasks(&data_dir(), &tasks)
        .map_err(|e| anyhow::anyhow!("Failed to save tasks.yml: {}", e))?;

    Ok((format!("🗑️ Deleted cron job `{}`", job_id), false))
}

// ---------------------------------------------------------------------------
// Tool: update_cron_job
// ---------------------------------------------------------------------------

async fn handle_update(pool: &PgPool, args: &Value) -> Result<(String, bool)> {
    let job_id = args["job_id"].as_str().unwrap_or("");
    if job_id.is_empty() {
        return Err(anyhow::anyhow!("Missing required argument: 'job_id'"));
    }

    let mut tasks = tasks_yaml::load_tasks_or_empty(&data_dir());

    if !tasks.schedules.contains_key(job_id) {
        return Err(anyhow::anyhow!("Cron job `{}` not found", job_id));
    }

    let mut changed: Vec<String> = Vec::new();

    // `name` IS the yml key (the job id): a changed name RE-KEYS the schedule
    // and re-points the thread/task_runs bookkeeping, exactly like the API's
    // PATCH /schedule/{id} (parity fix 2026-09-15). Clearing it is rejected -
    // the job would lose its identity.
    let mut effective_id = job_id.to_string();
    if args.get("name").is_some() {
        let new_name = tri_state_str(args, "name")
            .flatten()
            .ok_or_else(|| anyhow::anyhow!("'name' cannot be cleared (it is the job id)"))?;
        let new_key = generate_id(&new_name);
        if !new_key.is_empty() && new_key != job_id {
            if tasks.schedules.contains_key(&new_key) {
                return Err(anyhow::anyhow!(
                    "A schedule named '{}' already exists",
                    new_key
                ));
            }
            let moved = tasks.schedules.remove(job_id).expect("schedule exists");
            tasks.schedules.insert(new_key.clone(), moved);
            if let Err(e) =
                sqlx::query("UPDATE threads SET schedule_task_id = $1 WHERE schedule_task_id = $2")
                    .bind(&new_key)
                    .bind(job_id)
                    .execute(pool)
                    .await
            {
                tracing::error!(
                    "[cron {}] rename: thread reference update failed: {e:?}",
                    job_id
                );
            }
            if let Err(e) = sqlx::query("UPDATE task_runs SET task_key = $1 WHERE task_key = $2")
                .bind(&new_key)
                .bind(job_id)
                .execute(pool)
                .await
            {
                tracing::error!("[cron {}] rename: task_runs update failed: {e:?}", job_id);
            }
            effective_id = new_key;
            changed.push("name".to_string());
        }
    }

    let entry = tasks
        .schedules
        .get_mut(&effective_id)
        .ok_or_else(|| anyhow::anyhow!("Cron job `{}` not found", job_id))?;

    // cron: `schedule` is the historical tool name, `cron` the API field name.
    if let Some(schedule) = args["schedule"].as_str().or_else(|| args["cron"].as_str()) {
        validate_5field(schedule)?;
        entry.cron = schedule.to_string();
        changed.push("schedule".to_string());
    }

    if let Some(prompt) = tri_state_str(args, "prompt") {
        // A prompt switches the job back to agentic mode (mirrors the API).
        entry.prompt = prompt;
        if entry.prompt.is_some() {
            entry.action = None;
        }
        changed.push("prompt".to_string());
    }

    if let Some(channel) =
        tri_state_str(args, "channel").or_else(|| tri_state_str(args, "channel_id"))
    {
        entry.channel = match channel {
            Some(cid) => match channel_name_for_id(pool, &cid).await {
                Some(name) => Some(name),
                None => return Err(anyhow::anyhow!("Unknown channel '{}'", cid)),
            },
            None => None,
        };
        changed.push("channel".to_string());
    }

    if let Some(profile) = tri_state_str(args, "profile") {
        entry.profile = profile;
        changed.push("profile".to_string());
    }

    if let Some(mode) = args["mode"].as_str() {
        match mode {
            "agentic" => {
                entry.action = None;
                changed.push("mode=agentic".to_string());
            }
            "action" => {
                let aid = tri_state_str(args, "action_id")
                    .flatten()
                    .or_else(|| entry.action.clone())
                    .ok_or_else(|| anyhow::anyhow!("action_id is required for action mode"))?;
                entry.action = Some(aid);
                changed.push("mode=action".to_string());
            }
            other => {
                return Err(anyhow::anyhow!(
                    "Invalid mode '{}'. Must be 'agentic' or 'action'",
                    other
                ))
            }
        }
    } else if let Some(action_id) = tri_state_str(args, "action_id") {
        entry.action = action_id;
        changed.push("action_id".to_string());
    }

    if let Some(silent) = args["silent"].as_bool() {
        entry.silent = Some(silent);
        changed.push("silent".to_string());
    }
    if let Some(active) = args["active"]
        .as_bool()
        .or_else(|| args["enabled"].as_bool())
    {
        entry.enabled = active;
        changed.push(
            if active {
                "active=true"
            } else {
                "active=false"
            }
            .to_string(),
        );
    }
    if let Some(template) = tri_state_str(args, "template") {
        entry.template = template;
        changed.push("template".to_string());
    }
    if let Some(toolset) = tri_state_str(args, "toolset") {
        entry.toolset = toolset;
        changed.push("toolset".to_string());
    }
    if let Some(plan) = tri_state_bool(args, "plan") {
        entry.plan = plan;
        changed.push("plan".to_string());
    }
    if let Some(skills) = args.get("skills").and_then(|v| v.as_str()) {
        let parts: Vec<String> = skills
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        entry.skills = if parts.is_empty() {
            None
        } else {
            Some(serde_json::json!(parts).to_string())
        };
        changed.push("skills".to_string());
    }

    if changed.is_empty() {
        return Err(anyhow::anyhow!(
            "No updatable field provided (name, schedule, prompt, channel, profile, mode, action_id, silent, active, template, toolset, plan, skills)"
        ));
    }

    tasks_yaml::save_tasks(&data_dir(), &tasks)
        .map_err(|e| anyhow::anyhow!("Failed to save tasks.yml: {}", e))?;

    Ok((
        format!(
            "✅ Updated cron job `{}` ({})",
            effective_id,
            changed.join(", ")
        ),
        false,
    ))
}

// ---------------------------------------------------------------------------
// Plugin config hook
// ---------------------------------------------------------------------------

/// Plugin config - received via configure message.
#[derive(Debug, Clone)]
struct PluginConfig {
    pub database_url: String,
}

impl PluginConfig {
    fn from_json(v: &serde_json::Value) -> Self {
        Self {
            database_url: v
                .get("database_url")
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_else(|| {
                    eprintln!("FATAL: database_url not in configure message");
                    std::process::exit(1);
                }),
        }
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    // Shared pool - populated by configure callback before any tool call
    // Channels live in {OMNI_DIR}/config/channels.yml - set the global data dir.
    omniagent::channels_yaml::set_data_dir(&data_dir());
    let pool = Arc::new(RwLock::new(None::<PgPool>));

    // Wrap each handler to capture a clone of the shared pool
    let p_cron = pool.clone();
    let create_handler: ToolHandler = Box::new(move |args: Value, meta: Option<McpMeta>| {
        let p = p_cron.clone();
        Box::pin(async move {
            let guard = p.read().await;
            let pool = guard.as_ref().expect("Pool not initialized").clone();
            handle_create(&pool, &args, meta.as_ref()).await
        })
    });
    let p_list = pool.clone();
    let list_handler: ToolHandler = Box::new(move |args: Value, _meta: Option<McpMeta>| {
        let p = p_list.clone();
        Box::pin(async move {
            let guard = p.read().await;
            let pool = guard.as_ref().expect("Pool not initialized").clone();
            handle_list(&pool, &args).await
        })
    });
    let p_del = pool.clone();
    let delete_handler: ToolHandler = Box::new(move |args: Value, _meta: Option<McpMeta>| {
        let p = p_del.clone();
        Box::pin(async move {
            let guard = p.read().await;
            let pool = guard.as_ref().expect("Pool not initialized").clone();
            handle_delete(&pool, &args).await
        })
    });
    let p_upd = pool.clone();
    let update_handler: ToolHandler = Box::new(move |args: Value, _meta: Option<McpMeta>| {
        let p = p_upd.clone();
        Box::pin(async move {
            let guard = p.read().await;
            let pool = guard.as_ref().expect("Pool not initialized").clone();
            handle_update(&pool, &args).await
        })
    });

    let tools = vec![
        McpToolEntry {
            def: McpToolDef {
                name: "create_cron_job".to_string(),
                description:
                    "Create a new cron job (full parity with POST /schedule). Schedules a recurring task with a cron expression and a prompt to execute. The 'name' is the job's identifier in the schedule section and is editable. Writes the same tasks.yml source of truth the schedule API uses.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "A unique short name for this cron job (lowercase, underscores, no spaces)" },
                        "schedule": { "type": "string", "description": "Cron schedule expression in 5-field Linux format (min hour day month weekday)" },
                        "prompt": { "type": "string", "description": "The prompt/message to execute when the cron job triggers" },
                        "skills": { "type": "string", "description": "Optional comma-separated list of skill names" },
                        "channel": { "type": "string", "description": "Optional channel name (the API field name; default: current channel)" },
                        "channel_id": { "type": "string", "description": "Legacy alias for channel" },
                        "profile": { "type": "string", "description": "Optional profile name (default: current profile)" },
                        "mode": { "type": "string", "description": "Job mode: 'agentic' (default) or 'action'" },
                        "action_id": { "type": "string", "description": "For mode='action': the action ID to execute" },
                        "silent": { "type": "boolean", "description": "When true and mode='action', no thread/messages on success" },
                        "template": { "type": "string", "description": "Optional template name for the spawned thread" },
                        "toolset": { "type": "string", "description": "Optional toolset key for the spawned thread" },
                        "plan": { "type": "boolean", "description": "Plan mode for the spawned thread" },
                        "active": { "type": "boolean", "description": "Whether the job is enabled (default true); alias of enabled" },
                        "enabled": { "type": "boolean", "description": "Whether the job is enabled (default true); alias of active" },
                    },
                    "required": ["name", "schedule"],
                }),
            },
            handler: create_handler,
        },
        McpToolEntry {
            def: McpToolDef {
                name: "list_cron_jobs".to_string(),
                description: "List all cron jobs with their schedule, mode, channel, toolset/template/plan and status. Runs are visible via the threads each job creates. Optional 'active' filter maps to GET /schedule?active=.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "active": { "type": "boolean", "description": "Only return jobs with this enabled state (parity with GET /schedule?active=)" },
                        "enabled": { "type": "boolean", "description": "Alias of active" },
                    },
                    "required": [],
                }),
            },
            handler: list_handler,
        },
        McpToolEntry {
            def: McpToolDef {
                name: "delete_cron_job".to_string(),
                description: "Delete a cron job by its job_id.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "job_id": { "type": "string", "description": "The ID of the cron job to delete" },
                    },
                    "required": ["job_id"],
                }),
            },
            handler: delete_handler,
        },
        McpToolEntry {
            def: McpToolDef {
                name: "update_cron_job".to_string(),
                description: "Update a cron job (full parity with PATCH /schedule/{id}). String fields are tri-state: omit to keep, pass null or an empty string to clear, pass a value to set. 'prompt' switches the job to agentic mode; mode='action' plus action_id switches it to action mode. A changed 'name' re-keys the job (the name IS the job id), like the API.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "job_id": { "type": "string", "description": "The ID of the cron job to update" },
                        "name": { "type": "string", "description": "New job name; the name IS the job id, so this re-keys the schedule and re-points its thread/task_runs bookkeeping (same as PATCH /schedule/{id}). Cannot be cleared." },
                        "schedule": { "type": "string", "description": "New cron schedule in 5-field format" },
                        "cron": { "type": "string", "description": "Alias of schedule (API field name)" },
                        "prompt": { "type": "string", "description": "New prompt (switches mode to agentic); null clears it" },
                        "channel": { "type": "string", "description": "New channel name; null clears it" },
                        "profile": { "type": "string", "description": "New profile; null clears it" },
                        "mode": { "type": "string", "description": "'agentic' or 'action' (action requires action_id)" },
                        "action_id": { "type": "string", "description": "Action id for action mode; null clears it" },
                        "silent": { "type": "boolean", "description": "Silent mode for action jobs" },
                        "active": { "type": "boolean", "description": "Set to true/false to activate/deactivate" },
                        "enabled": { "type": "boolean", "description": "Alias of active" },
                        "template": { "type": "string", "description": "Template for the spawned thread; null clears it" },
                        "toolset": { "type": "string", "description": "Toolset for the spawned thread; null clears it" },
                        "plan": { "type": "boolean", "description": "Plan mode; null clears it" },
                        "skills": { "type": "string", "description": "Comma-separated skills; empty string clears them" },
                    },
                    "required": ["job_id"],
                }),
            },
            handler: update_handler,
        },
    ];

    let server_info = ServerInfo {
        name: "mcp-server-cron".to_string(),
        version: "0.1.0".to_string(),
    };

    run_server_with_config(server_info, tools, {
        let p = pool.clone();
        Some(move |params: serde_json::Value| {
            let config = PluginConfig::from_json(&params);
            tokio::task::block_in_place(|| {
                let rt = tokio::runtime::Handle::current();
                let new_pool = rt
                    .block_on(db::connect(&config.database_url))
                    .expect("Failed to connect to database");
                *p.blocking_write() = Some(new_pool);
            });
            tracing::info!("Cron plugin configured with database_url");
        })
    })
    .await
}
