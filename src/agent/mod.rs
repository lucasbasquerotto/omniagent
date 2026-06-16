//! Agent module — core agent loop for processing messages across channels.
//!
//! The agent uses a simple state machine per message:
//! - `pending`  → agent picks it up
//! - `processing` → LLM is being called
//! - `completed` → response generated and saved
//! - `failed`    → something went wrong
//!
//! On startup, any message stuck in `processing` for >5 minutes is
//! automatically marked as `failed` (recovery).

use anyhow::Result;
use sqlx::PgPool;
use tokio::time::{sleep, Duration};
use tracing::{info, error, warn};

use crate::db::queries;
use crate::models::{Message, MessageNew, MessageStatus};

/// Configuration for the agent's LLM interactions.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub llm_api_key: String,
    pub llm_model: String,
    pub llm_provider: String,
    pub max_tokens: u32,
    pub temperature: f32,
    pub summarize_after_days: u32,
}

impl AgentConfig {
    /// Load agent configuration from environment variables.
    ///
    /// # Env vars
    /// - `LLM_API_KEY` — API key for the LLM provider
    /// - `LLM_MODEL` — Model name (default: "gpt-4")
    /// - `LLM_PROVIDER` — Provider name (default: "openai")
    /// - `MAX_TOKENS` — Max tokens per response (default: 4096)
    /// - `TEMPERATURE` — Sampling temperature (default: 0.7)
    /// - `SUMMARIZE_AFTER_DAYS` — Days before auto-summarization (default: 7)
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            llm_api_key: std::env::var("LLM_API_KEY")
                .unwrap_or_default(),
            llm_model: std::env::var("LLM_MODEL")
                .unwrap_or_else(|_| "gpt-4".to_string()),
            llm_provider: std::env::var("LLM_PROVIDER")
                .unwrap_or_else(|_| "openai".to_string()),
            max_tokens: std::env::var("MAX_TOKENS")
                .unwrap_or_else(|_| "4096".to_string())
                .parse()
                .unwrap_or(4096),
            temperature: std::env::var("TEMPERATURE")
                .unwrap_or_else(|_| "0.7".to_string())
                .parse()
                .unwrap_or(0.7),
            summarize_after_days: std::env::var("SUMMARIZE_AFTER_DAYS")
                .unwrap_or_else(|_| "7".to_string())
                .parse()
                .unwrap_or(7),
        })
    }
}

/// The core agent that processes incoming messages across all channels.
pub struct Agent {
    pub pool: PgPool,
    pub config: AgentConfig,
}

impl Agent {
    pub fn new(pool: PgPool, config: AgentConfig) -> Self {
        Self { pool, config }
    }

    /// Run the main agent loop.
    ///
    /// This loop:
    /// 1. Recovers stale `processing` messages on startup.
    /// 2. Continuously polls all channels for the oldest `pending` message.
    /// 3. Processes each message via [`process_message`].
    /// 4. Sleeps 1 second between channels to avoid hammering the DB.
    pub async fn run(&self) -> Result<()> {
        // Recover any messages stuck in 'processing' for >5 minutes
        recover_stale_processing(&self.pool).await?;

        loop {
            let channels = queries::find_all_channels(&self.pool).await?;

            if channels.is_empty() {
                info!("No channels found, sleeping...");
                sleep(Duration::from_secs(5)).await;
                continue;
            }

            for channel in &channels {
                let pending = queries::find_pending_messages(&self.pool, channel.id).await?;

                if let Some(msg) = pending.into_iter().next() {
                    info!(
                        "Processing message {} in channel '{}' (thread: {})",
                        msg.id, channel.name, msg.thread_id
                    );

                    match self.process_message(msg).await {
                        Ok(response) => {
                            info!(
                                "Processed message, response id: {} (seq {})",
                                response.id, response.thread_sequence
                            );
                        }
                        Err(e) => {
                            error!("Failed to process message in channel '{}': {:?}", channel.name, e);
                        }
                    }
                }

                // Brief pause between channels to rate-limit DB queries
                sleep(Duration::from_secs(1)).await;
            }

            // Main loop tick
            sleep(Duration::from_secs(1)).await;
        }
    }

    /// Process a single pending message through the state machine:
    ///
    /// 1. Update message status → `processing`
    /// 2. Create an agent response (status: `completed`, same thread, seq+1)
    /// 3. Update original message status → `completed`
    /// 4. Return the saved response message
    ///
    /// Currently the response content is a stub (echoes the user input).
    /// Real LLM integration will replace this later.
    async fn process_message(&self, msg: Message) -> Result<Message> {
        // 1. Mark the message as 'processing'
        queries::update_message_status(&self.pool, msg.id, &MessageStatus::Processing).await?;

        // 2. Create the agent's response message
        let response = MessageNew {
            channel_id: msg.channel_id,
            role: "agent".to_string(),
            content: format!("Received: {}", msg.content),
            status: MessageStatus::Completed,
            thread_id: msg.thread_id,
            thread_sequence: msg.thread_sequence + 1,
            external_id: None,
            metadata: serde_json::json!({}),
            embedding: None,
            summary_text: None,
            is_summary: false,
        };

        let saved = queries::create_message(&self.pool, &response).await?;

        // 3. Mark the original message as 'completed'
        queries::update_message_status(&self.pool, msg.id, &MessageStatus::Completed).await?;

        Ok(saved)
    }
}

/// On startup, find any messages that are still `processing` but were created
/// more than 5 minutes ago — mark them as `failed` to unblock the channel.
///
/// Returns the number of recovered messages.
pub async fn recover_stale_processing(pool: &PgPool) -> Result<u64> {
    let five_min_ago = chrono::Utc::now() - chrono::Duration::minutes(5);
    let stale = queries::find_processing_older_than(pool, five_min_ago).await?;
    let count = stale.len() as u64;

    for msg in &stale {
        warn!(
            "Recovering stale processing message {} (created at {})",
            msg.id, msg.created_at
        );
        queries::update_message_status(pool, msg.id, &MessageStatus::Failed).await?;
    }

    if count > 0 {
        info!("Recovered {} stale processing messages", count);
    }

    Ok(count)
}
