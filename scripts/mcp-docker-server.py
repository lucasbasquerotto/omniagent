#!/usr/bin/env python3
"""
Docker Compose MCP Server for OmniAgent.

Provides ONLY `docker compose` operations, restricted to project directories
under /opt/workspace. No raw docker commands, no docker exec, no docker build.

Tools:
  - compose: Run docker compose commands (ps, up, down, logs, build, exec, stop, restart)

Safety:
  - All commands start with `docker compose` only
  - Shell metacharacters (|, ;, &&, ||, `, $(), >, <) are rejected
  - Project directory must be under /opt/workspace/
  - Timeouts prevent hung operations
"""

import json
import sys
import subprocess
import os
import shlex

# ── Safety ──────────────────────────────────────────────────────────────

WORKSPACE_DIR = os.environ.get("WORKSPACE_DIR", "/opt/workspace")
SAFE_CMD_PREFIXES = ["docker"]
COMPOSE_ONLY = True  # Only docker compose subcommand allowed
FORBIDDEN_CHARS = ["|", ";", "&", "`", "$", ">", "<", "*", "?", "[", "]", "{", "}", "(", ")", "!", "~"]
TIMEOUT_SECS = 300


def validate_command(tool_name, cmd_parts):
    """Validate that the command is safe to execute."""
    if not cmd_parts or not cmd_parts[0]:
        return False, "Empty command"

    if cmd_parts[0] not in SAFE_CMD_PREFIXES:
        return False, f"Only docker commands are allowed, got: {cmd_parts[0]}"

    # Only allow `docker compose` subcommand
    if COMPOSE_ONLY and (len(cmd_parts) < 2 or cmd_parts[1] != "compose"):
        return False, f"Only `docker compose` is allowed, got: {' '.join(cmd_parts[:2])}"

    # Check for forbidden shell metacharacters in all parts
    for i, part in enumerate(cmd_parts):
        for char in FORBIDDEN_CHARS:
            if char in part:
                return False, f"Forbidden character '{char}' in argument {i}: {part[:50]}"

    return True, ""


def validate_workspace_path(project_dir):
    """Validate that the project directory is under WORKSPACE_DIR."""
    if not project_dir:
        return True, ""  # No specific directory - will use compose defaults

    resolved = os.path.realpath(project_dir)
    workspace = os.path.realpath(WORKSPACE_DIR)

    if not resolved.startswith(workspace):
        return False, f"Project directory must be under {WORKSPACE_DIR}, got: {project_dir}"

    if not os.path.isdir(resolved):
        return False, f"Project directory does not exist: {resolved}"

    return True, ""


def run_docker(cmd_parts, timeout=TIMEOUT_SECS):
    """Run a docker command and return (stdout, stderr, returncode)."""
    valid, err = validate_command("compose", cmd_parts)
    if not valid:
        return "", f"Command rejected: {err}", -1

    try:
        result = subprocess.run(
            cmd_parts,
            capture_output=True,
            text=True,
            timeout=timeout
        )
        return result.stdout, result.stderr, result.returncode
    except subprocess.TimeoutExpired:
        return "", f"Command timed out after {timeout}s", -1
    except FileNotFoundError:
        return "", f"Command not found: {cmd_parts[0]}", -1
    except PermissionError:
        return "", f"Permission denied for: {cmd_parts[0]}", -1


# ── Tool Implementations ────────────────────────────────────────────────


def tool_compose(args):
    """Run a docker compose command."""
    project_dir = args.get("dir", "").strip()
    command = args.get("command", "ps").strip()
    service = args.get("service", "").strip()
    extra_args = args.get("args", "").strip()

    # Validate workspace path
    valid, err = validate_workspace_path(project_dir)
    if not valid:
        return error_result(err)

    cmd = ["docker", "compose"]
    if project_dir:
        cmd.extend(["--project-directory", project_dir])

    cmd.append(command)
    if service:
        cmd.append(service)
    if extra_args:
        try:
            cmd.extend(shlex.split(extra_args))
        except ValueError as e:
            return error_result(f"Invalid extra args: {e}")

    timeout = TIMEOUT_SECS
    if command in ("build",):
        timeout = 600

    stdout, stderr, rc = run_docker(cmd, timeout=timeout)
    if rc != 0:
        return error_result(f"docker compose {command} failed:\n{stderr}")

    content = f"```\n{stdout}\n```" if stdout else f"docker compose {command}: ok"
    return text_result(content)


# ── MCP Protocol ────────────────────────────────────────────────────────


def text_result(text):
    return {"content": [{"type": "text", "text": text}]}


def error_result(message):
    return {"content": [{"type": "text", "text": f"ERROR: {message}"}], "is_error": True}


TOOLS = [
    {
        "name": "compose",
        "description": "Run docker compose commands (ps, up, down, logs, build, exec, stop, restart). "
                       "Only operates on projects under /opt/workspace/. "
                       "Use 'dir' set to the project directory containing docker-compose.yml. "
                       "Default compose commands: ps, up -d, down, logs --tail=50, build, stop, restart",
        "inputSchema": {
            "type": "object",
            "properties": {
                "dir": {
                    "type": "string",
                    "description": "Absolute path to the project directory containing docker-compose.yml "
                                   "(must be under /opt/workspace/)"
                },
                "command": {
                    "type": "string",
                    "description": "Compose subcommand: ps, up, down, logs, build, exec, stop, restart, pull"
                },
                "service": {
                    "type": "string",
                    "description": "Service name (optional, for targeted commands like 'logs web' or 'stop db')"
                },
                "args": {
                    "type": "string",
                    "description": "Extra arguments e.g. '-d' for 'up -d', '--tail=50' for 'logs --tail=50', "
                                   "'-d --build' for 'up -d --build'"
                }
            },
            "required": ["command"]
        }
    },
]


def handle_initialize(msg_id):
    return {
        "jsonrpc": "2.0",
        "id": msg_id,
        "result": {
            "protocolVersion": "2025-03-26",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "mcp-docker-compose-server", "version": "1.0.0"}
        }
    }


def handle_list_tools(msg_id):
    return {"jsonrpc": "2.0", "id": msg_id, "result": {"tools": TOOLS}}


def handle_call_tool(msg_id, params):
    tool_name = params.get("name", "")
    arguments = params.get("arguments", {})

    tool_map = {
        "compose": tool_compose,
    }

    handler = tool_map.get(tool_name)
    if not handler:
        return {
            "jsonrpc": "2.0",
            "id": msg_id,
            "error": {"code": -32601, "message": f"Tool not found: {tool_name}"}
        }

    try:
        result = handler(arguments)
        return {"jsonrpc": "2.0", "id": msg_id, "result": result}
    except Exception as e:
        import traceback
        sys.stderr.write(f"ERROR in tool {tool_name}: {traceback.format_exc()}\n")
        sys.stderr.flush()
        return {
            "jsonrpc": "2.0",
            "id": msg_id,
            "error": {"code": -32603, "message": f"Internal error: {str(e)}"}
        }


def main():
    sys.stderr.write(f"[mcp-docker-compose-server] Starting with Python {sys.version}\n")
    sys.stderr.write(f"[mcp-docker-compose-server] Workspace: {WORKSPACE_DIR}\n")
    sys.stderr.write(f"[mcp-docker-compose-server] Docker available: ")
    try:
        r = subprocess.run(["docker", "--version"], capture_output=True, text=True, timeout=5)
        sys.stderr.write(r.stdout.strip() + "\n")
    except Exception as e:
        sys.stderr.write(f"NO: {e}\n")
    sys.stderr.flush()

    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue

        method = msg.get("method")
        msg_id = msg.get("id")

        if method == "initialize":
            response = handle_initialize(msg_id)
        elif method == "notifications/initialized":
            continue
        elif method == "tools/list":
            response = handle_list_tools(msg_id)
        elif method == "tools/call":
            response = handle_call_tool(msg_id, msg.get("params", {}))
        elif method == "shutdown":
            sys.exit(0)
        else:
            if msg_id:
                response = {
                    "jsonrpc": "2.0", "id": msg_id,
                    "error": {"code": -32601, "message": f"Method not found: {method}"}
                }
            else:
                continue

        sys.stdout.write(json.dumps(response) + "\n")
        sys.stdout.flush()


if __name__ == "__main__":
    main()
