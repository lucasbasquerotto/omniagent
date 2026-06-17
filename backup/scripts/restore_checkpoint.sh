#!/bin/bash
# Restore checkpoint: sync from S3 checkpoint to /opt/data/ + Postgres + Qdrant
# Usage: restore_checkpoint YYYYMMDD
set -euo pipefail

: "${S3_BUCKET:?S3_BUCKET not set}"
: "${S3_PATH:?S3_PATH not set}"

export RCLONE_CONFIG=${RCLONE_CONFIG:-/etc/rclone/rclone.conf}

if [ $# -lt 1 ]; then
    echo "Usage: restore_checkpoint YYYYMMDD"
    echo "Example: restore_checkpoint 20260616"
    exit 1
fi

DATE_SUFFIX="$1"
if ! echo "$DATE_SUFFIX" | grep -qE '^[0-9]{8}$'; then
    echo "Error: Date must be in YYYYMMDD format (got: $DATE_SUFFIX)"
    exit 1
fi

SRC="s3-backup:${S3_BUCKET}/${S3_PATH}/checkpoint/${DATE_SUFFIX}"

echo "[restore_checkpoint] Starting full restore from checkpoint ${DATE_SUFFIX}..."

# ─── 1. File data ─────────────────────────────────────────────────────
echo "[restore_checkpoint] Step 1/3: File data ← ${SRC}/data/"
rclone sync "${SRC}/data/" /opt/data/ --create-empty-src-dirs --s3-no-check-bucket --verbose

# ─── 2. Postgres restore ──────────────────────────────────────────────
echo "[restore_checkpoint] Step 2/3: Postgres restore..."
PG_HOST="${PGHOST:-postgres}"
PG_PORT="${PGPORT:-5432}"
PG_USER="${PGUSER:-omniagent}"
PG_DB="${PGDATABASE:-omniagent}"

if [ -n "${PGPASSWORD:-}" ]; then
  export PGPASSWORD

  if rclone ls "${SRC}/db/pg-dump.sql.gz" &>/dev/null; then
    echo "[restore_checkpoint] Downloading Postgres dump from S3..."
    rclone copy "${SRC}/db/pg-dump.sql.gz" /tmp/ --s3-no-check-bucket

    echo "[restore_checkpoint] Terminating connections to $PG_DB..."
    psql -h "$PG_HOST" -p "$PG_PORT" -U "$PG_USER" -d postgres -c "
      SELECT pg_terminate_backend(pg_stat_activity.pid)
      FROM pg_stat_activity
      WHERE pg_stat_activity.datname = '$PG_DB'
        AND pid <> pg_backend_pid();
    " 2>&1

    echo "[restore_checkpoint] Dropping and recreating database $PG_DB..."
    psql -h "$PG_HOST" -p "$PG_PORT" -U "$PG_USER" -d postgres \
      -c "DROP DATABASE IF EXISTS $PG_DB;" 2>&1
    psql -h "$PG_HOST" -p "$PG_PORT" -U "$PG_USER" -d postgres \
      -c "CREATE DATABASE $PG_DB;" 2>&1

    echo "[restore_checkpoint] Restoring Postgres from dump..."
    gunzip -c /tmp/pg-dump.sql.gz | psql -h "$PG_HOST" -p "$PG_PORT" \
      -U "$PG_USER" -d "$PG_DB" 2>&1
    rm -f /tmp/pg-dump.sql.gz

    echo "[restore_checkpoint] Postgres restore complete."
  else
    echo "[restore_checkpoint] No Postgres dump at ${SRC}/db/pg-dump.sql.gz — skipping."
  fi
else
  echo "[restore_checkpoint] PGPASSWORD not set — skipping Postgres restore."
fi

# ─── 3. Qdrant restore ────────────────────────────────────────────────
echo "[restore_checkpoint] Step 3/3: Qdrant restore..."

if rclone ls "${SRC}/db/wiki-snapshot.snapshot" &>/dev/null; then
  echo "[restore_checkpoint] Downloading Qdrant snapshot from S3..."
  rclone copy "${SRC}/db/wiki-snapshot.snapshot" /tmp/ --s3-no-check-bucket

  if [ -f /tmp/wiki-snapshot.snapshot ] && [ -s /tmp/wiki-snapshot.snapshot ]; then
    echo "[restore_checkpoint] Uploading snapshot to Qdrant..."
    curl -s -X POST \
      -F "snapshot=@/tmp/wiki-snapshot.snapshot" \
      "http://qdrant:6333/collections/wiki/snapshots/upload?priority=snapshot" || {
      echo "[restore_checkpoint] Qdrant snapshot upload failed — the collection may not exist yet."
    }
    rm -f /tmp/wiki-snapshot.snapshot
    echo "[restore_checkpoint] Qdrant restore complete."
  fi
else
  echo "[restore_checkpoint] No Qdrant snapshot at ${SRC}/db/wiki-snapshot.snapshot — skipping."
fi

echo "[restore_checkpoint] Full restore from checkpoint ${DATE_SUFFIX} complete."
