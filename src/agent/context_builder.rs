use crate::error::AppResult;
use tracing::info;

use crate::agent::config::AgentContext;
use crate::agent::plugin_manager::PluginManager;
use crate::agent::prompt_sections::{
    assemble, parse_plugin_sections, parse_template_frontmatter, PromptSection,
};
use crate::db::types::{Channel, Message, Thread};
use crate::llm::ChatMessage;
use crate::mcp::{AppContext, McpToolCall};
use serde::{Deserialize, Serialize};
use sql_forge::sql_forge;
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;

/// structured-message template. Returns the prompt parts and optional template section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PromptParts {
    pub system: String,
    pub memory: String,
    pub context: String,
    pub user: String,
    pub plan: bool,
}

/// Dependencies of the ONE prompt-assembly path.
///
/// Deliberately narrower than `AgentContext`: the read-only HTTP preview
/// routes (dashboard Prompt/Memory pages) must reuse the EXACT executor
/// assembly but do not own an LLM client. Every field is a borrow or a
/// snapshot, so the executor and the HTTP routes share one implementation.
pub(crate) struct PromptAssemblyDeps<'a> {
    pub data_dir: &'a str,
    pub pool: &'a PgPool,
    pub plugin_manager: &'a Arc<dyn PluginManager>,
    pub app_context: &'a AppContext,
    /// The configured prompt tool name (settings `prompt_generate_tool`,
    /// default `prompt__generate`).
    pub prompt_tool_name: String,
    /// Persist the plugin's plan decision into `threads.plan`.
    /// TRUE for the executor (the decision is binding), FALSE for the
    /// dashboard preview (`prompt-preview` is documented as no-DB-writes).
    pub persist_plan: bool,
}

impl<'a> PromptAssemblyDeps<'a> {
    /// Executor entry: everything comes from the live `AgentContext`.
    pub(crate) fn from_agent_context(cfg: &'a AgentContext) -> Self {
        Self {
            data_dir: &cfg.ctx.data_dir,
            pool: &cfg.pool,
            plugin_manager: &cfg.plugin_manager,
            app_context: &cfg.ctx,
            prompt_tool_name: cfg.config_snapshot().prompt_tool_name,
            persist_plan: true,
        }
    }
}

/// Assemble the INITIAL message list from the prompt parts.
///
/// THE single implementation of the prompt layout: the executor's main loop
/// and the dashboard prompt preview both call it, so a preview can never
/// diverge from the real prompt. `plan_text` is `None` for a read-only
/// preview (no LLM planning call is made there).
pub(crate) fn initial_prompt_messages(
    parts: &PromptParts,
    template_section: Option<&str>,
    plan_text: Option<&str>,
    is_step_thread: bool,
) -> Vec<ChatMessage> {
    let mut messages = vec![ChatMessage::system(&parts.system)];
    if !parts.memory.is_empty() {
        messages.push(ChatMessage::system(&parts.memory));
    }

    // Inject task template FIRST (right after system prompt): highest instruction priority
    // for template-backed tasks (kanban/cron with template).
    // Flush-left position ensures the template guides the model before any other context.
    // For step threads the template is deferred to the USER slot (see below).
    if let Some(template_section) = template_section {
        if !is_step_thread {
            messages.push(ChatMessage::system(template_section));
        }
    }

    // Add context from plugin as system message (before the user message)
    if !parts.context.is_empty() {
        messages.push(ChatMessage::system(&format!(
            "=== Context ===\n{}",
            parts.context
        )));
    }

    // Inject the plan as execution context if one was generated
    if let Some(plan) = plan_text {
        messages.push(ChatMessage::system(&format!(
            "=== Generated Plan (use as guidance) ===\n\
             A plan was generated for the current task. Follow it unless tool results \
             contradict it. Do NOT explore alternative approaches that the plan already \
             considered: adapt only when necessary.\n\n{}",
            plan
        )));
        info!("[plan] Injected plan as context ({} chars)", plan.len());
    }

    // Step threads: task description goes in the SYSTEM slot, the role
    // template in the USER slot (inverse of the executor layout).
    if is_step_thread {
        messages.push(ChatMessage::system(&format!(
            "=== Task Description ===\n{}",
            parts.user
        )));
        match template_section {
            Some(template_section) => messages.push(ChatMessage::user(template_section)),
            None => messages.push(ChatMessage::user(&parts.user)),
        }
    } else {
        // Add the user message (from the prompt parts: the plugin provides this)
        messages.push(ChatMessage::user(&parts.user));
    }

    messages
}

/// Helper: serialize the assembled messages for JSON APIs (dashboard preview).
pub(crate) fn messages_to_json(messages: &[ChatMessage]) -> Vec<serde_json::Value> {
    messages
        .iter()
        .map(|m| serde_json::json!({ "role": m.role, "content": m.content }))
        .collect()
}

/// Resolve the thread template name. The thread record is the single source
/// of truth (threads.template, populated by the kanban dispatcher, cron
/// scheduler, and message handler alike). The seq-0 cause message metadata
/// is the fallback for threads created before the column existed or where
/// the creator did not set a template (uniform template resolution, R7).
fn resolve_template_name(thread: &Thread, cause_msg: &Message) -> Option<String> {
    thread
        .template
        .as_deref()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            cause_msg
                .metadata
                .get("template")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
        })
        .map(|s| s.to_string())
}

/// Absolute path of a thread template file for a profile.
fn template_path(data_dir: &str, profile_name: &str, template: &str) -> std::path::PathBuf {
    let file = if template.ends_with(".md") || template.contains('.') {
        template.to_string()
    } else {
        format!("{}.md", template)
    };
    std::path::PathBuf::from(data_dir)
        .join("profiles")
        .join(profile_name)
        .join("templates")
        .join(file)
}

/// Channel-scoped prompt sections (task 9): `prompt_sections` from the
/// channel's channels.yml definition. Keyed by channel NAME (the stable
/// identifier); falls back to channel id for legacy rows where they differ.
fn load_channel_sections(data_dir: &str, channel: &Channel) -> AppResult<Vec<PromptSection>> {
    let file = crate::channels_yaml::load_channels_from(data_dir)?;
    let by_name = file.channels.get(&channel.name);
    let def = by_name.or_else(|| {
        if channel.id.is_empty() || channel.id == channel.name {
            None
        } else {
            file.channels.get(&channel.id)
        }
    });
    Ok(def
        .and_then(|d| d.prompt_sections.clone())
        .unwrap_or_default())
}

/// Build the `{{variable}}` registry for section interpolation.
fn build_variables(
    profile_name: &str,
    thread: &Thread,
    channel: &Channel,
    tool_names: &[String],
    template_name: &Option<String>,
) -> HashMap<String, String> {
    let mut variables = HashMap::new();
    variables.insert("profile_name".to_string(), profile_name.to_string());
    variables.insert("channel".to_string(), channel.name.clone());
    variables.insert("channel_id".to_string(), channel.id.clone());
    variables.insert("thread_id".to_string(), thread.id.to_string());
    variables.insert(
        "platform".to_string(),
        channel.platform.clone().unwrap_or_default(),
    );
    variables.insert("tools".to_string(), tool_names.join(", "));
    variables.insert(
        "template".to_string(),
        template_name.clone().unwrap_or_default(),
    );
    variables
}

/// Wrap a template body in the standard Task Template block (exactly the
/// format used before task 9, so legacy rendering stays byte-identical).
fn wrap_template(body: &str) -> String {
    format!(
        "=== Task Template ===\nThe following template provides structured guidance for this task type:\n\n{}",
        body
    )
}

/// THE ONE prompt-assembly path: calls the configured prompt tool
/// (settings `prompt_generate_tool`, default `prompt__generate`) and
/// assembles the prompt parts exactly as the executor consumes them.
///
/// Both the agent executor (`build_prompt_context`) and the read-only HTTP
/// preview routes (dashboard Prompt/Memory pages) call this function, so a
/// preview can never diverge from the real prompt. `deps.persist_plan`
/// controls the ONLY side effect (writing the plugin's plan decision to
/// `threads.plan`); previews pass `false` (documented no-DB-writes).
pub(crate) async fn assemble_prompt(
    deps: &PromptAssemblyDeps<'_>,
    thread: &Thread,
    cause_msg: &Message,
    channel: &Channel,
    profile_name: &str,
    tool_names: &[String],
) -> AppResult<(PromptParts, Option<String>)> {
    let template_name = resolve_template_name(thread, cause_msg);

    // ── Call the configured prompt plugin (sys-prompt-gen) ──
    // V-5: the platform plugin OWNS its formatting hint; it advertises it as
    // `capabilities.prompt_hint` in its initialize result and the core
    // forwards it to the prompt tool. A platform that declares nothing sends
    // null and the prompt tool uses its generic markdown fallback.
    let platform_name = channel.platform.as_deref().unwrap_or("");
    let platform_hint =
        crate::agent::helpers::platform_prompt_hint(deps.app_context, platform_name).await;

    let (parsed, plan) = {
        let prompt_tool_name = deps.prompt_tool_name.clone();
        let mcp_call = McpToolCall {
            id: "sys-prompt-gen".to_string(),
            name: prompt_tool_name,
            arguments: serde_json::json!({
                "profile_name": profile_name,
                "platform": platform_name,
                "platform_hint": platform_hint,
                "user_message": cause_msg.content,
                "tool_names": tool_names,
                "thread_id": thread.id,
                "channel_id": thread.channel_id,
                // Only an explicit plan=true forces planning. For anything
                // else (false or undecided) pass null so the prompt plugin's
                // complexity config decides at runtime.
                "plan": if thread.plan {
                    serde_json::Value::Bool(true)
                } else {
                    serde_json::Value::Null
                },
            }),
        };
        let result = deps
            .plugin_manager
            .snapshot_registry()
            .await
            .execute(&mcp_call, deps.app_context.clone())
            .await?;
        if result.is_error {
            return Err(crate::error::Error::Message(format!(
                "prompt generation tool failed: {}",
                result.content
            )));
        }
        let parsed: serde_json::Value = serde_json::from_str(&result.content).map_err(|e| {
            crate::error::Error::Message(format!("prompt generation returned invalid JSON: {e}"))
        })?;

        // If the plugin returned a plan decision, persist it to the thread.
        // Skipped for read-only previews (deps.persist_plan == false): the
        // preview reports the decision WITHOUT writing it.
        if deps.persist_plan && parsed.get("plan").is_some() {
            let plan_val = parsed["plan"].as_bool().unwrap_or(false);
            sql_forge!(
                "UPDATE threads SET plan = :plan WHERE id = :thread_id",
                ( :plan = plan_val, :thread_id = thread.id )
            )
            .execute(deps.pool)
            .await?;
        }
        let plan = parsed
            .get("plan")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        (parsed, plan)
    };

    // ── Raw template body (frontmatter-aware) ──
    let template_raw: Option<String> = match &template_name {
        Some(template) => {
            let path = template_path(deps.data_dir, profile_name, template);
            if path.exists() {
                std::fs::read_to_string(&path)
                    .ok()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .inspect(|content| {
                        info!(
                            "Loaded template '{}' for thread {} ({} chars)",
                            template,
                            thread.id,
                            content.len()
                        );
                    })
            } else {
                None
            }
        }
        None => None,
    };

    // ── Task 9: ordered/scoped prompt sections ──
    // The prompt plugin MAY return `sections: [{name, order, text}]` in
    // addition to (or instead of) the legacy flat fields. When present, the
    // core assembles the SYSTEM prompt from the sections: sorted by
    // ascending `order`, with per-thread SCOPE SHADOWING (template sections
    // shadow channel sections, which shadow the plugin's global sections)
    // and `{{variable}}` interpolation. When absent, the legacy flat fields
    // render exactly as before (backward compatible, byte-identical).
    let parsed_sections = parse_plugin_sections(&parsed)?;
    let (system, template_section) = match parsed_sections {
        Some(plugin_sections) => {
            let variables =
                build_variables(profile_name, thread, channel, tool_names, &template_name);
            let channel_sections = load_channel_sections(deps.data_dir, channel)?;
            let (template_scoped, template_body) = match &template_raw {
                Some(raw) => {
                    let (fm_sections, body) = parse_template_frontmatter(raw)?;
                    let body = body.trim().to_string();
                    (fm_sections, (!body.is_empty()).then_some(body))
                }
                None => (Vec::new(), None),
            };
            // A template-declared `task_template` section fully takes over
            // the template slot (the body is NOT injected separately).
            let takeover = template_scoped.iter().any(|s| s.name == "task_template");
            let assembled = assemble(
                &[plugin_sections, channel_sections, template_scoped],
                &variables,
            )?;
            let system = if assembled.is_empty() {
                // Sections mode produced nothing: keep the plugin's flat
                // `system` field as a graceful fallback.
                parsed["system"].as_str().unwrap_or("").to_string()
            } else {
                assembled
            };
            let template_section = match template_body {
                Some(body) if !takeover => Some(wrap_template(&body)),
                _ => None,
            };
            (system, template_section)
        }
        // Legacy mode: no `sections` in the response → the flat fields and
        // the template block render byte-identical to pre-task-9.
        None => {
            let system = parsed["system"].as_str().unwrap_or("").to_string();
            let template_section = match &template_raw {
                Some(raw) => {
                    let (_, body) = parse_template_frontmatter(raw)?;
                    let body = body.trim().to_string();
                    if body.is_empty() {
                        None
                    } else {
                        Some(wrap_template(&body))
                    }
                }
                None => None,
            };
            (system, template_section)
        }
    };

    let prompt_parts = PromptParts {
        system,
        memory: parsed["memory"].as_str().unwrap_or("").to_string(),
        context: parsed["context"].as_str().unwrap_or("").to_string(),
        user: parsed["user"].as_str().unwrap_or("").to_string(),
        plan,
    };

    Ok((prompt_parts, template_section))
}

/// Executor entry point: the SAME assembly with plan persistence ON.
pub(crate) async fn build_prompt_context(
    cfg: &AgentContext,
    thread: &Thread,
    cause_msg: &Message,
    channel: &Channel,
    profile_name: &str,
    tool_names: &[String],
) -> AppResult<(PromptParts, Option<String>)> {
    let deps = PromptAssemblyDeps::from_agent_context(cfg);
    assemble_prompt(&deps, thread, cause_msg, channel, profile_name, tool_names).await
}
