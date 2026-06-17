#!/bin/bash
# OmniAgent — first-boot startup script
# Runs inside the Vagrant VM, invoked by the Vagrantfile provisioner.
set -euo pipefail

OMNI_HOME="/opt/data"
REPO_DIR="/opt/omniagent"
LOG="$OMNI_HOME/logs/startup_$(date +%Y%m%d_%H%M%S).log"
mkdir -p "$OMNI_HOME/logs"

echo "[$(date '+%Y-%m-%d %H:%M:%S')] OmniAgent startup script — begin" | tee -a "$LOG"

# ── 1. Create /opt/data if missing ──────────────────────────────────
mkdir -p "$OMNI_HOME"

# ── 2. Source env if present ────────────────────────────────────────
if [ -f "$OMNI_HOME/.env" ]; then
  set -a
  source "$OMNI_HOME/.env"
  set +a
fi

# ── 3. Start Docker containers ───────────────────────────────────────
if command -v docker &>/dev/null && [ -f "$REPO_DIR/docker-compose.yml" ]; then
  cd "$REPO_DIR"
  echo "[$(date '+%Y-%m-%d %H:%M:%S')] Starting OmniAgent containers..." | tee -a "$LOG"
  docker compose up -d 2>&1 | tee -a "$LOG"

  # ── 4. Wait for postgres ────────────────────────────────────────────
  echo "[$(date '+%Y-%m-%d %H:%M:%S')] Waiting for postgres to be healthy..." | tee -a "$LOG"
  for i in $(seq 1 30); do
    if docker compose exec -T postgres pg_isready -U omniagent -d omniagent &>/dev/null; then
      echo "[$(date '+%Y-%m-%d %H:%M:%S')] Postgres is ready!" | tee -a "$LOG"
      break
    fi
    sleep 2
  done

  # ── 5. Stop omniagent (so restore doesn't conflict with connections) ─
  echo "[$(date '+%Y-%m-%d %H:%M:%S')] Stopping omniagent for restore..." | tee -a "$LOG"
  docker compose stop omniagent 2>&1 | tee -a "$LOG"

  # ── 6. Restore from S3 backup ───────────────────────────────────────
  echo "[$(date '+%Y-%m-%d %H:%M:%S')] Running restore from S3 backup..." | tee -a "$LOG"
  timeout 300 docker compose exec -T backup restore_backup 2>&1 | tee -a "$LOG" || true

  # ── 7. Start omniagent ──────────────────────────────────────────────
  echo "[$(date '+%Y-%m-%d %H:%M:%S')] Starting omniagent..." | tee -a "$LOG"
  docker compose start omniagent 2>&1 | tee -a "$LOG"

  echo "[$(date '+%Y-%m-%d %H:%M:%S')] OmniAgent containers are running." | tee -a "$LOG"
else
  echo "[$(date '+%Y-%m-%d %H:%M:%S')] Docker or compose file not found — start containers manually." | tee -a "$LOG"
fi

echo "[$(date '+%Y-%m-%d %H:%M:%S')] OmniAgent startup script — complete" | tee -a "$LOG"
