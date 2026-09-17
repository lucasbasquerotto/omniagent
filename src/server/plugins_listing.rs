//! Plugin listing and discovery handlers.
//!
//! Extracted from `plugins.rs` for separation of concerns.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use sql_forge::sql_forge;
use std::sync::Arc;
use tracing::error;

use crate::err_str;
use crate::plugins_yaml;
use crate::server::AppState;

pub(crate) async fn list_plugins_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let data_dir = state.data_dir.clone();
    match tokio::task::spawn_blocking(move || plugins_yaml::list_plugins(&data_dir))
        .await
        .unwrap_or_else(|e| Err(err_str!("Task join error: {}", e)))
    {
        Ok(mut details) => {
            // Resolve $secret: references in resolved_env for all plugins
            for detail in details.iter_mut() {
                for val in detail.resolved_env.values_mut() {
                    if let Some(secret_name) = val.strip_prefix("$secret:") {
                        let lookup = sql_forge!(
                            String,
                            "SELECT current_value FROM secrets WHERE name = :name",
                            ( :name = secret_name )
                        )
                        .fetch_optional(&state.pool)
                        .await;
                        match lookup {
                            Ok(Some(secret_val)) => {
                                *val = secret_val;
                            }
                            Ok(None) => {
                                tracing::warn!(
                                    "Secret '{}' referenced in plugin config but not found in DB",
                                    secret_name
                                );
                            }
                            Err(e) => {
                                tracing::error!(
                                    "DB error looking up secret '{}': {:?}",
                                    secret_name,
                                    e
                                );
                            }
                        }
                    }
                }
            }

            // Cross-reference MCP plugins with the MCP registry:
            // Live runtime status: tool_names from the MCP registry plus a
            // TRUTHFUL error message when an enabled plugin is not running.
            // Shared with the detail endpoint so both always agree.
            super::plugins_reload::apply_tool_runtime_status_all(&state, &mut details).await;

            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "success": true,
                    "data": details
                })),
            )
                .into_response()
        }
        Err(e) => {
            error!("Failed to list plugins: {:?}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "success": false,
                    "error": format!("Failed to list plugins: {}", e)
                })),
            )
                .into_response()
        }
    }
}

/// GET /api/plugins/{type}/{source}/{name}: get single plugin detail.
pub(crate) async fn get_plugin_handler(
    Path((p_type, _source, name)): Path<(String, String, String)>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let data_dir = state.data_dir.clone();
    let name_clone = name.clone();
    let p_type_clone = p_type.clone();
    match tokio::task::spawn_blocking(move || {
        let pt = crate::plugins_yaml::PluginYamlType::from_type_str(&p_type_clone);
        plugins_yaml::get_plugin(&data_dir, &name_clone, &pt)
    })
    .await
    .unwrap_or_else(|e| Err(err_str!("Task join error: {}", e)))
    {
        Ok(Some(mut detail)) => {
            // Resolve $secret: references in resolved_env
            for val in detail.resolved_env.values_mut() {
                if let Some(secret_name) = val.strip_prefix("$secret:") {
                    let lookup = sql_forge!(
                        String,
                        "SELECT current_value FROM secrets WHERE name = :name",
                        ( :name = secret_name )
                    )
                    .fetch_optional(&state.pool)
                    .await;
                    match lookup {
                        Ok(Some(secret_val)) => {
                            *val = secret_val;
                        }
                        Ok(None) => {
                            tracing::warn!(
                                "Secret '{}' referenced in plugin config but not found in DB",
                                secret_name
                            );
                        }
                        Err(e) => {
                            tracing::error!(
                                "DB error looking up secret '{}': {:?}",
                                secret_name,
                                e
                            );
                        }
                    }
                }
            }

            // Live runtime status: the SAME helper as the list endpoint, so a
            // plugin can never be "enabled" in one response and "error" in the
            // other (and tool_names is populated here too).
            super::plugins_reload::apply_tool_runtime_status(&state, &mut detail).await;

            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "success": true,
                    "data": detail
                })),
            )
                .into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "success": false,
                "error": "Plugin not found"
            })),
        )
            .into_response(),
        Err(e) => {
            error!("Failed to get plugin '{}': {:?}", name, e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "success": false,
                    "error": format!("Failed to get plugin: {}", e)
                })),
            )
                .into_response()
        }
    }
}
