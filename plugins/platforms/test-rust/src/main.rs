use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing_subscriber::EnvFilter;

// ---------------------------------------------------------------------------
// Protocol types
// ---------------------------------------------------------------------------

/// Incoming JSON-RPC-like request from the agent.
#[derive(Debug, Deserialize)]
struct Request {
    id: u64,
    method: String,
    #[serde(default)]
    params: Value,
}

/// Successful response.
#[derive(Debug, Serialize)]
struct Response {
    id: u64,
    result: Value,
}

/// Error response.
#[derive(Debug, Serialize)]
struct ErrorResponse {
    id: u64,
    error: ErrorDetail,
}

#[derive(Debug, Serialize)]
struct ErrorDetail {
    code: i64,
    message: String,
}

/// Platform capabilities advertised during initialization.
#[derive(Debug, Serialize)]
struct Capabilities {
    inbound: bool,
    outbound: bool,
}

/// Initialize result.
#[derive(Debug, Serialize)]
struct InitializeResult {
    name: String,
    capabilities: Capabilities,
}

/// Deliver result.
#[derive(Debug, Serialize)]
struct DeliverResult {
    delivered: bool,
    external_id: String,
}

/// Edit message result.
#[derive(Debug, Serialize)]
struct EditResult {
    edited: bool,
}

/// Delete message result.
#[derive(Debug, Serialize)]
struct DeleteResult {
    deleted: bool,
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing — log to stderr
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    tracing::info!("test-rust platform plugin starting");

    let stdin = tokio::io::stdin();
    let reader = BufReader::new(stdin);
    let mut lines = reader.lines();

    let stdout = tokio::io::stdout();
    let mut writer = tokio::io::BufWriter::new(stdout);

    let mut message_counter: u64 = 0;

    while let Some(line) = lines.next_line().await? {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        // Parse the incoming request
        let request: Request = match serde_json::from_str(&line) {
            Ok(req) => req,
            Err(e) => {
                tracing::error!("Failed to parse request: {e}");
                continue;
            }
        };

        let req_id = request.id;
        let method = request.method.as_str();

        tracing::info!("Received method='{method}' id={req_id}");

        match method {
            "initialize" => {
                handle_initialize(&mut writer, req_id).await?;
            }
            "deliver" => {
                message_counter += 1;
                handle_deliver(&mut writer, req_id, &request.params, message_counter).await?;
            }
            "edit_message" => {
                handle_edit_message(&mut writer, req_id, &request.params).await?;
            }
            "delete_message" => {
                handle_delete_message(&mut writer, req_id, &request.params).await?;
            }
            _ => {
                tracing::warn!("Unknown method: {method}");
                let error = ErrorResponse {
                    id: req_id,
                    error: ErrorDetail {
                        code: -1,
                        message: format!("Unknown method: {method}"),
                    },
                };
                let json = serde_json::to_string(&error)?;
                writer.write_all(json.as_bytes()).await?;
                writer.write_all(b"\n").await?;
                writer.flush().await?;
            }
        }
    }

    tracing::info!("test-rust platform plugin shutting down (stdin closed)");
    Ok(())
}

// ---------------------------------------------------------------------------
// Method handlers
// ---------------------------------------------------------------------------

async fn handle_initialize<W: AsyncWriteExt + Unpin>(
    writer: &mut tokio::io::BufWriter<W>,
    req_id: u64,
) -> Result<()> {
    let result = InitializeResult {
        name: "test-rust".to_string(),
        capabilities: Capabilities {
            inbound: false,
            outbound: true,
        },
    };

    let response = Response {
        id: req_id,
        result: serde_json::to_value(result)?,
    };

    let json = serde_json::to_string(&response)?;
    writer.write_all(json.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;

    tracing::info!("Initialized: test-rust");
    Ok(())
}

async fn handle_deliver<W: AsyncWriteExt + Unpin>(
    writer: &mut tokio::io::BufWriter<W>,
    req_id: u64,
    params: &Value,
    msg_num: u64,
) -> Result<()> {
    let resource = params
        .get("resource_identifier")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let content = params
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let msg_type = params
        .get("msg_type")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    tracing::info!(
        "Deliver [{msg_num}] to {resource} (type={msg_type}): {content}",
        content = &content[..content.len().min(80)]
    );

    let external_id = format!("test-rust-{msg_num}");
    let result = DeliverResult {
        delivered: true,
        external_id,
    };

    let response = Response {
        id: req_id,
        result: serde_json::to_value(result)?,
    };

    let json = serde_json::to_string(&response)?;
    writer.write_all(json.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;

    Ok(())
}

async fn handle_edit_message<W: AsyncWriteExt + Unpin>(
    writer: &mut tokio::io::BufWriter<W>,
    req_id: u64,
    params: &Value,
) -> Result<()> {
    let resource = params
        .get("resource_identifier")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let external_id = params
        .get("external_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let content = params
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    tracing::info!(
        "Edit message {external_id} in {resource}: {content}",
        content = &content[..content.len().min(80)]
    );

    let result = EditResult { edited: true };

    let response = Response {
        id: req_id,
        result: serde_json::to_value(result)?,
    };

    let json = serde_json::to_string(&response)?;
    writer.write_all(json.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;

    Ok(())
}

async fn handle_delete_message<W: AsyncWriteExt + Unpin>(
    writer: &mut tokio::io::BufWriter<W>,
    req_id: u64,
    params: &Value,
) -> Result<()> {
    let resource = params
        .get("resource_identifier")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let external_id = params
        .get("external_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    tracing::info!("Delete message {external_id} in {resource}");

    let result = DeleteResult { deleted: true };

    let response = Response {
        id: req_id,
        result: serde_json::to_value(result)?,
    };

    let json = serde_json::to_string(&response)?;
    writer.write_all(json.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;

    Ok(())
}
