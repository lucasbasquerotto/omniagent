//! HTTP server for external control (stop, health, etc.)
//!
//! Provides endpoints:
//! - `GET /health` — health check
//! - `POST /stop/{channel_id}` — stop processing for a channel
//! - `GET /stop/{channel_id}` — same (for easier testing)

use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use crate::db::queries;

/// Shared application state for the HTTP server.
#[derive(Clone)]
struct AppState {
    pool: PgPool,
    cancel_tokens: Arc<Mutex<HashMap<i64, CancellationToken>>>,
}

/// Start the HTTP server on the given host and port.
///
/// The server provides endpoints for health checking and stopping
/// channel processing. The `cancel_tokens` map is shared with the
/// agent supervisor so channels can be cleanly stopped.
pub async fn start_server(
    pool: PgPool,
    host: String,
    port: u16,
    cancel_tokens: Arc<Mutex<HashMap<i64, CancellationToken>>>,
) {
    let app_state = Arc::new(AppState { pool, cancel_tokens });

    let app = Router::new()
        .route("/health", get(health_handler))
        .route("/stop/{channel_id}", post(stop_handler))
        .route("/stop/{channel_id}", get(stop_handler))
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
