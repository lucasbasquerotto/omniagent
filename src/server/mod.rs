//! HTTP server for external control (stop, health, etc.)
//!
//! Provides endpoints:
//! - `GET /health` — health check
//! - `POST /stop/{channel_id}` — stop processing for a channel
//! - `GET /stop/{channel_id}` — same (for easier testing)

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use crate::db::types as queries;
use crate::prompt_builder::{build_system_prompt, MemoryStore};

/// Shared application state for the HTTP server.
#[derive(Clone)]
struct AppState {
    pool: PgPool,
    cancel_tokens: Arc<Mutex<HashMap<i64, CancellationToken>>>,
    data_dir: String,
}

/// Start the HTTP server on the given host and port.
pub async fn start_server(
    pool: PgPool,
    host: String,
    port: u16,
    cancel_tokens: Arc<Mutex<HashMap<i64, CancellationToken>>>,
    data_dir: String,
) {
    let app_state = Arc::new(AppState {
        pool,
        cancel_tokens,
        data_dir,
    });

    let app = Router::new()
        .route("/health", get(health_handler))
        .route("/stop/:channel_id", post(stop_handler))
        .route("/stop/:channel_id", get(stop_handler))
        .route("/prompt/:channel_name", get(prompt_handler))
        .with_state(app_state);

    let addr = format!("{}:{}", host, port);
    info!("Starting HTTP server on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("Failed to bind HTTP server address");

    axum::serve(listener, app)
        .await
        .expect("HTTP server exited with error");
}

/// Simple health check — returns "ok".
async fn health_handler() -> &'static str {
    "ok"
}

/// Stop processing for a specific channel.
///
/// This will:
/// 1. Mark all pending messages for the channel as `skipped`.
/// 2. Record the stop in the `channel_stops` table.
/// 3. Cancel the channel's processing task (if active).
async fn stop_handler(
    Path(channel_id): Path<i64>,
    State(state): State<Arc<AppState>>,
) -> Json<serde_json::Value> {
    // 1. Mark all pending messages as skipped
    match queries::skip_pending_messages(&state.pool, channel_id).await {
        Ok(count) => {
            info!(
                "Skipped {} pending messages for channel {}",
                count, channel_id
            );
        }
        Err(e) => {
            error!(
                "Failed to skip messages for channel {}: {:?}",
                channel_id, e
            );
        }
    }

    // 2. Record the stop in the database
    if let Err(e) = queries::stop_channel(&state.pool, channel_id).await {
        error!("Failed to record stop for channel {}: {:?}", channel_id, e);
    }

    // 3. Cancel the channel's processing task (if running)
    let mut tokens = state.cancel_tokens.lock().await;
    if let Some(token) = tokens.remove(&channel_id) {
        token.cancel();
        info!("Cancelled processing task for channel {}", channel_id);
        Json(serde_json::json!({
            "status": "stopped",
            "channel_id": channel_id,
        }))
    } else {
        Json(serde_json::json!({
            "status": "no_active_task",
            "channel_id": channel_id,
        }))
    }
}

/// Show the system prompt for a channel, using `<<<prompt>>>` as the
/// placeholder for where the user's actual message would go.
///
/// This is a reference tool — it shows what the prompt preamble would
/// look like when a user sends a message to this channel, without
/// actually invoking the agent.
async fn prompt_handler(
    Path(channel_name): Path<String>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    // 1. Look up the channel by name
    let channel = match queries::get_channel_by_name(&state.pool, &channel_name).await {
        Ok(Some(ch)) => ch,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                format!("Channel '{}' not found", channel_name),
            );
        }
        Err(e) => {
            error!("Failed to look up channel '{}': {:?}", channel_name, e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Database error: {}", e),
            );
        }
    };

    // 2. Determine the profile name
    let profile_name = if channel.current_profile.is_empty() {
        "default"
    } else {
        &channel.current_profile
    };

    // 3. Load the memory store for the profile
    let profile_path = format!("{}/profiles/{}", state.data_dir, profile_name);
    let mut memory_store = MemoryStore::new(&profile_path);
    memory_store.load_from_disk();

    // 4. Build the system prompt (same as process_message would)
    let platform = channel.platform.as_str();
    let system_prompt = build_system_prompt(&memory_store, platform, None, profile_name);

    // 5. Format the full messages array as it would be sent to the LLM
    let result = format!(
        "System Prompt:\n{}\n\n---\n\nMessages sent to LLM:\n\n{{\n  \"role\": \"system\",\n  \"content\": \"\"\"\n{}\n  \"\"\"\n}},\n{{\n  \"role\": \"user\",\n  \"content\": \"<<<prompt>>>\"\n}}",
        system_prompt, system_prompt
    );

    (StatusCode::OK, result)
}
