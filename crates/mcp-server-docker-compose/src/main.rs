//! mcp-server-docker-compose — standalone MCP server for docker compose commands.
//! Communicates via stdio JSON-RPC (MCP protocol).
//!
//! Tools: docker_compose

use anyhow::Result;
use mcp_server_util::*;
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

/// Characters that are forbidden in arguments to prevent shell injection.
const FORBIDDEN_CHARS: &[char] = &['|', ';', '&', '`', '$', '>', '<', '*', '?', '[', ']', '{', '}', '!', '~'];

/// Validate that a string contains no shell-metacharacters.
fn contains_forbidden_chars(s: &str) -> bool {
    s.chars().any(|c| FORBIDDEN_CHARS.contains(&c))
}

/// Validate that a project directory is under the allowed workspace.
fn validate_workspace_path(project_dir: &str, workspace_dir: &str) -> Result<()> {
    if project_dir.is_empty() {
        return Ok(());
    }
    let resolved = Path::new(project_dir)
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("Invalid project directory '{}': {}", project_dir, e))?;
    let workspace = Path::new(workspace_dir)
        .canonicalize()
        .unwrap_or_else(|_| Path::new(workspace_dir).to_path_buf());
    if !resolved.starts_with(&workspace) {
        anyhow::bail!(
            "Project directory must be under {}, got: {}",
            workspace_dir,
            project_dir
        );
    }
    if !resolved.is_dir() {
        anyhow::bail!("Project directory does not exist: {}", resolved.display());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tool: docker_compose
// ---------------------------------------------------------------------------

fn handle_compose(args: &Value) -> Result<(String, bool)> {
    let workspace_dir = std::env::var("WORKSPACE_DIR")
        .unwrap_or_else(|_| "/opt/workspace".to_string());

    let command = args["command"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing 'command' argument"))?
        .to_string();

    if contains_forbidden_chars(&command) {
        anyhow::bail!("Forbidden characters in command argument");
    }

    let project_dir = args["project_dir"].as_str().unwrap_or("").to_string();
    if contains_forbidden_chars(&project_dir) {
        anyhow::bail!("Forbidden characters in project_dir argument");
    }

    if !project_dir.is_empty() {
        validate_workspace_path(&project_dir, &workspace_dir)?;
    }

    // Build arguments: `docker compose <command>`
    let mut cmd = std::process::Command::new("docker");
    cmd.arg("compose");

    // Split the command string to support e.g. "up -d", "logs --tail=50"
    for part in command.split_whitespace() {
        if !part.is_empty() {
            if contains_forbidden_chars(part) {
                anyhow::bail!("Forbidden characters in command argument");
            }
            cmd.arg(part);
        }
    }

    if !project_dir.is_empty() {
        cmd.current_dir(&project_dir);
    }

    let timeout_secs = if command.starts_with("build") { 600u64 } else { 300u64 };

    // Use std::process::Command + mpsc for timeout (same pattern as pre-ext code)
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = cmd.output();
        let _ = tx.send(result);
    });

    match rx.recv_timeout(Duration::from_secs(timeout_secs)) {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let rc = output.status.code().unwrap_or(-1);

            if rc != 0 {
                let msg = if stderr.is_empty() {
                    format!("docker compose command failed (exit {}):\n{}", rc, stdout)
                } else {
                    format!("docker compose command failed (exit {}):\n{}", rc, stderr)
                };
                return Ok((msg, true));
            }

            let content = if stdout.is_empty() {
                format!("docker compose {}: ok", command)
            } else {
                // Truncate
                let max_chars: usize = 50_000;
                if stdout.len() > max_chars {
                    format!(
                        "```\n{}\n```\n\n[... truncated from {} to ~{} chars]",
                        &stdout[..max_chars],
                        stdout.len(),
                        max_chars
                    )
                } else {
                    format!("```\n{}\n```", stdout)
                }
            };

            Ok((content, false))
        }
        Ok(Err(e)) => Ok((format!("docker command failed: {}", e), true)),
        Err(_) => Ok((format!("docker command timed out after {}s", timeout_secs), true)),
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    let compose_handler: ToolHandler = Box::new(|args: &Value| handle_compose(args));

    let tools = vec![McpToolEntry {
        def: McpToolDef {
            name: "docker_compose".to_string(),
            description:
                "Run docker compose commands (up, down, ps, logs, exec, build, restart, stop). \
                 Use 'project_dir' to set the directory containing docker-compose.yml. \
                 Use 'command' for the compose subcommand and arguments (e.g. 'up -d', 'ps', 'logs --tail=50')."
                    .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "project_dir": {
                        "type": "string",
                        "description": "Directory containing docker-compose.yml"
                    },
                    "command": {
                        "type": "string",
                        "description": "Docker compose command and arguments (e.g. 'up -d', 'ps', 'logs --tail=50')"
                    }
                },
                "required": ["project_dir", "command"]
            }),
        },
        handler: compose_handler,
    }];

    let server_info = ServerInfo {
        name: "mcp-server-docker-compose".to_string(),
        version: "0.1.0".to_string(),
    };

    run_server(server_info, tools).await
}
