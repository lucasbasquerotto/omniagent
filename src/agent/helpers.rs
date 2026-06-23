use sqlx::PgPool;
use tracing::{error, info, warn};

use crate::agent::config::AgentConfig;
use crate::db::types as queries;
use crate::db::types::{Channel, CompleteThreadStats, Message, MessageNew, Thread};
use crate::llm::{ChatMessage, CompletionRequest, LLMClient, Usage};
use crate::mcp::{AppContext};
use crate::platform::queue::OutboundEnvelope;
use crate::platform::enqueue_notification;

/// Maximum total characters of tool results in conversation history before
/// old tool results are pruned (Layer 3 compression).
const TOOL_RESULT_HISTORY_BUDGET: usize = 120_000;

/// Merge cumulative usage with a new usage value.
pub fn merge_usage(cumulative: &mut Option<Usage>, new_usage: Option<Usage>) {
    if let Some(new) = new_usage {
        if let Some(ref mut cum) = cumulative {
            cum.prompt_tokens += new.prompt_tokens;
            cum.completion_tokens += new.completion_tokens;
            cum.cached_tokens =
                Some(cum.cached_tokens.unwrap_or(0) + new.cached_tokens.unwrap_or(0));
            cum.reasoning_tokens = cum.reasoning_tokens.or(new.reasoning_tokens);
        } else {
            *cumulative = Some(new);
        }
    }
}

/// Check if a database error is a foreign key violation (PostgreSQL code 23503).
/// These indicate the thread was deleted or the FK constraint was broken —
/// the thread should be marked as failed rather than retried.
fn is_fk_violation(e: &anyhow::Error) -> bool {
    if let Some(sqlx::Error::Database(ref dberr)) = e.downcast_ref::<sqlx::Error>() {
            return dberr.code().as_deref() == Some("23503");
    }
    false
}

/// Persist a message and detect FK violations that should abort thread processing.
/// Returns the created message on success, or an error variant.
pub enum CreateMessageResult {
    Success(Message),
    FkViolation,
    OtherError(anyhow::Error),
}

pub async fn persist_or_abort(
    pool: &PgPool,
    msg: &MessageNew,
    thread_id: i64,
) -> CreateMessageResult {
    match queries::create_message(pool, msg).await {
        Ok(saved) => CreateMessageResult::Success(saved),
        Err(e) if is_fk_violation(&e) => {
            error!(
                "FK violation inserting message for thread {} — marking thread as failed",
                thread_id
            );
            // Mark the thread as failed
            let _ = queries::complete_thread(pool, thread_id, "failed", CompleteThreadStats { input_tokens: 0, cached_tokens: 0, output_tokens: 0, duration_ms: 0 }).await;
            CreateMessageResult::FkViolation
        }
        Err(e) => CreateMessageResult::OtherError(e),
    }
}

/// Prune old tool results from the conversation history when the total
///
/// Keeps the most recent turn's results intact and strips old tool result
/// bodies, replacing them with a short summary, while preserving all
/// user, assistant, and system messages unchanged.
pub fn prune_old_tool_results(messages: &mut [ChatMessage]) {
    let total_tool_chars: usize = messages
        .iter()
        .filter(|m| m.role == "tool")
        .map(|m| m.content.len())
        .sum();

    if total_tool_chars <= TOOL_RESULT_HISTORY_BUDGET {
        return;
    }

    // Find the index of the last assistant message with tool_calls — this
    // marks the most recent turn boundary. Tool results after it are kept.
    let last_tool_turn_idx = messages
        .iter()
        .rposition(|m| m.role == "assistant" && m.tool_calls.is_some());

    let keep_from = last_tool_turn_idx.unwrap_or(0);

    for msg in messages.iter_mut().take(keep_from) {
        if msg.role == "tool" && msg.content.len() > 500 {
            let preview = if msg.content.len() > 200 {
                let truncate_to = msg
                    .content
                    .char_indices()
                    .nth(200)
                    .map(|(i, _)| i)
                    .unwrap_or(msg.content.len());
                format!("{}...", &msg.content[..truncate_to])
            } else {
                msg.content.clone()
            };
            msg.content = format!(
                "[Pruned tool result — was {} chars] {preview}",
                msg.content.len(),
            );
        }
    }
}

/// Check if enough completed threads have accumulated since the
/// last summary for this channel, and if so, generate a new cross-thread summary.
///
/// Algorithm:
/// 1. Get the `next_thread_id` from the latest summary (0 if none).
/// 2. Count completed threads with id > next_thread_id.
/// 3. If count >= 2*N (where N = SUMMARY_WINDOW), generate a summary.
/// 4. The first thread id = first thread, the last = last thread.
/// 5. For each of the 2*N threads, fetch ALL its messages.
/// 6. Build a summarization prompt with previous summary context.
/// 7. Save with `next_thread_id` = the N-th thread's id (window slides by N).
pub async fn check_and_generate_summary(
    pool: &PgPool,
    llm: &LLMClient,
    config: &AgentConfig,
    channel_id: i64,
) {
    let window = config.summary_window as i64;
    if window == 0 {
        return; // summaries disabled
    }
    let trigger_count = window * 2; // need 2*N threads to trigger

    // 1. Get latest summary's next_thread_id
    let since_id = match queries::get_latest_summary(pool, channel_id).await {
        Ok(Some(summary)) => summary.next_thread_id,
        _ => 0i64,
    };

    // 2. Fetch completed threads since the last summary
    let completed_threads = match queries::get_completed_seq0_threads_since(
        pool, channel_id, since_id, trigger_count,
    )
    .await
    {
        Ok(threads) => threads,
        Err(e) => {
            warn!(
                "[thread-summary] Failed to fetch completed threads for channel {}: {:?}",
                channel_id, e
            );
            return;
        }
    };

    if (completed_threads.len() as i64) < trigger_count {
        // Not enough threads yet
        return;
    }

    // We have 2*N threads. The first thread's id is completed_threads[0].id.
    // The N-th thread's id (the sliding window point):
    let pivot_thread_id = completed_threads[(window - 1) as usize].id;
    let first_thread_id = completed_threads[0].id;
    let last_thread_id = completed_threads[(trigger_count - 1) as usize].id;

    info!(
        "[thread-summary] Generating summary for channel {}: {} threads (id {} to {}), pivot={}",
        channel_id, trigger_count, first_thread_id, last_thread_id, pivot_thread_id
    );

    // 3. For each of the 2*N threads, fetch ALL messages
    let mut all_thread_content = String::new();
    for thread_db in &completed_threads {
        let tid = thread_db.id;
        match queries::get_thread_messages(pool, tid).await {
            Ok(thread_msgs) => {
                all_thread_content.push_str(&format!(
                    "\n=== Thread #{} (cause: {} at {}) ===\n",
                    tid,
                    thread_db.cause,
                    thread_db.created_at.as_deref().unwrap_or("?"),
                ));
                for m in &thread_msgs {
                    let role_display = match m.role.as_str() {
                        "user" => "User",
                        "agent" => "Assistant",
                        "system" => "System",
                        _ => &m.role,
                    };
                    // Skip tool results to keep context manageable
                    if m.msg_type == "tool_result" || m.msg_type == "tool" {
                        continue;
                    }
                    all_thread_content.push_str(&format!(
                        "[{}]: {}\n",
                        role_display,
                        m.content.chars().take(1000).collect::<String>()
                    ));
                }
            }
            Err(e) => {
                warn!(
                    "[thread-summary] Failed to fetch messages for thread {}: {:?}",
                    tid, e
                );
            }
        }
    }

    // 4. Fetch the last summary for context (to avoid repeating info)
    let previous_summary_text = match queries::get_latest_summary(pool, channel_id).await {
        Ok(Some(s)) => s.content,
        _ => String::new(),
    };

    // 5. Build summarization prompt — structured output
    //    The LLM produces a structured summary that can be parsed, searched, and
    //    cross-referenced with hindsight and Qdrant.
    let system_summarizer_prompt =
        "You are a conversation summarizer for an autonomous agent system. \
         Produce a structured summary in the exact format below. \
         Be specific — include file paths, config keys, exact numbers, and command names. \
         Do NOT repeat information covered in the previous summary (if provided). \
         Every claim must be grounded in the provided conversation content.\n\n\
         ## Format:\n\
         ### Topics\n\
         - topic: <topic_name> | detail: <one sentence with specifics>\n\n\
         ### Key Decisions\n\
         - decision: <what was decided> | context: <why> | files: <affected files, if any>\n\n\
         ### Action Items\n\
         - status: <done|pending|failed> | task: <what> | details: <specifics>\n\n\
         ### Entities Referenced\n\
         - <entity_name> (<type>): <relation to conversation>\n\n\
         ### Thread Count\n\
         - total: <number> | first: <id> | last: <id>\n\n\
         Keep each entry on a single line. Use | as field separator.";

    let summary_prompt = if previous_summary_text.is_empty() {
        format!(
            "Summarize the following conversations from a single channel.\n\n{}",
            all_thread_content
        )
    } else {
        format!(
            "PREVIOUS SUMMARY (do NOT repeat):\n{}\n\n---\n\n\
             Now summarize the following new conversations, \
             connecting to the previous summary if relevant.\n\n{}",
            previous_summary_text, all_thread_content
        )
    };

    // 6. Call LLM for summary
    let summary_request = CompletionRequest {
        messages: vec![
            ChatMessage::system(&system_summarizer_prompt),
            ChatMessage::user(&summary_prompt),
        ],
        max_tokens: config.summary_tokens,
        temperature: 0.2, // lower temperature for factual consistency
        stream: false,
        tools: None,
    };

    let summary_content = match llm.completion(summary_request).await {
        Ok(resp) => {
            info!(
                "[thread-summary] Generated summary for channel {} ({} chars, {} tokens)",
                channel_id,
                resp.content.len(),
                resp.usage.as_ref().map(|u| u.completion_tokens).unwrap_or(0),
            );
            resp.content
        }
        Err(e) => {
            warn!(
                "[thread-summary] Failed to generate summary for channel {}: {:?}",
                channel_id, e
            );
            return;
        }
    };

    // 7. Save the summary with next_thread_id = the N-th thread's id
    //    (window slides by N, so the next trigger will start from this pivot)
    match queries::create_summary(pool, channel_id, pivot_thread_id, &summary_content).await {
        Ok(summary) => {
            info!(
                "[thread-summary] Saved summary {} for channel {} (next_thread_id={}, covers {} threads)",
                summary.id, channel_id, pivot_thread_id, trigger_count
            );
        }
        Err(e) => {
            warn!(
                "[thread-summary] Failed to save summary for channel {}: {:?}",
                channel_id, e
            );
        }
    }
}

/// Enqueue a message for delivery to its platform.
pub async fn enqueue_delivery(
    ctx: &AppContext,
    saved: &Message,
    channel: &Channel,
    thread: &Thread,
    cause_external_id: Option<String>,
) {
    let platform = match &channel.platform {
        Some(p) => p.clone(),
        None => return,
    };
    let resource_identifier = match &channel.resource_identifier {
        Some(r) => r.clone(),
        None => return,
    };

    // Look up the per-platform sender
    let sender = match ctx.platform_senders.get(&platform) {
        Some(s) => s.clone(),
        None => return,
    };

    // For non-user threads, only deliver summaries and errors
    if thread.cause != "user" && saved.msg_type != "summary" && saved.msg_type != "error" {
        return;
    }

    // Never deliver tool results directly
    if saved.msg_type == "tool_result" {
        return;
    }

    let envelope_content = if saved.msg_type == "summary" && platform == "cli" {
        // Quote the seq-0 message for CLI delivery (not needed for Telegram — it uses reply threading)
        match queries::get_cause_message(&ctx.pool, saved.thread_id).await {
            Ok(Some(cause)) => {
                let cause_trimmed: String = cause.content.chars().take(100).collect();
                let quoted = if cause.content.len() > 100 {
                    format!("> {}...\n\n{}", cause_trimmed, saved.content)
                } else {
                    format!("> {}\n\n{}", cause_trimmed, saved.content)
                };
                quoted
            }
            _ => saved.content.clone(),
        }
    } else {
        saved.content.clone()
    };

    let envelope = OutboundEnvelope {
        message_id: saved.id,
        resource_identifier,
        content: envelope_content,
        msg_type: saved.msg_type.clone(),
        msg_subtype: saved.msg_subtype.clone(),
        thread_id: saved.thread_id,
        thread_sequence: saved.thread_sequence,
        cause_external_id,
        is_summary: saved.is_summary,
        is_user_thread: thread.cause == "user",
    };

    if let Err(e) = sender.try_send(envelope) {
        tracing::warn!("Failed to enqueue delivery for message {}: {:?}", saved.id, e);
    }

    // If this is a summary, also deliver to all subscribers of this channel
    if saved.msg_type == "summary" {
        let subscribers = queries::get_subscribers_for_channel(&ctx.pool, channel.id).await;
        if let Ok(subs) = subscribers {
            for sub in subs {
                tracing::info!(
                    "Forwarding summary from channel '{}' to subscriber {}:{}",
                    channel.name,
                    sub.subscriber_platform,
                    sub.subscriber_resource,
                );
                enqueue_notification(
                    &ctx.platform_senders,
                    &sub.subscriber_platform,
                    &sub.subscriber_resource,
                    &format!("[summary from {}]\n{}", channel.name, saved.content),
                );
            }
        }
    }
}
