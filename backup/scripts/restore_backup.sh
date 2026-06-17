#!/bin/bash
# Restore backup: sync from S3 to /opt/data/ + Postgres + Qdrant
set -euo pipefail

: "${S3_BUCKET:?S3_BUCKET not set}"
: "${S3_PATH:?S3_PATH not set}"

export RCLONE_CONFIG=${RCLONE_CONFIG:-/etc/rclone/rclone.conf}

SRC="s3-backup:${S3_BUCKET}/${S3_PATH}"

echo "[restore_backup] Starting full restore..."

# ─── 1. File data ─────────────────────────────────────────────────────
echo "[restore_backup] Step 1/3: File data ← ${SRC}/data/"
rclone sync "${SRC}/data/" /opt/data/ --create-empty-src-dirs --s3-no-check-bucket --verbose

# ─── 2. Postgres restore ──────────────────────────────────────────────
echo "[restore_backup] Step 2/3: Postgres restore..."
PG_HOST="${PGHOST:-postgres}"
PG_PORT="${PGPORT:-5432}"
PG_USER="${PGUSER:-omniagent}"
PG_DB="${PGDATABASE:-omniagent}"

if [ -n "${PGPASSWORD:-}" ]; then
  export PGPASSWORD

  # Check if a dump exists in S3
  if rclone ls "${SRC}/db/pg-dump.sql.gz" &>/dev/null; then
    echo "[restore_backup] Downloading Postgres dump from S3..."
    rclone copy "${SRC}/db/pg-dump.sql.gz" /tmp/ --s3-no-check-bucket

    echo "[restore_backup] Terminating connections to $PG_DB..."
    psql -h "$PG_HOST" -p "$PG_PORT" -U "$PG_USER" -d postgres -c "
      SELECT pg_terminate_backend(pg_stat_activity.pid)
      FROM pg_stat_activity
      WHERE pg_stat_activity.datname = '$PG_DB'
        AND pid <> pg_backend_pid();
    " 2>&1

    echo "[restore_backup] Dropping and recreating database $PG_DB..."
    psql -h "$PG_HOST" -p "$PG_PORT" -U "$PG_USER" -d postgres \
      -c "DROP DATABASE IF EXISTS $PG_DB;" 2>&1
    psql -h "$PG_HOST" -p "$PG_PORT" -U "$PG_USER" -d postgres \
      -c "CREATE DATABASE $PG_DB;" 2>&1

    echo "[restore_backup] Restoring Postgres from dump..."
    gunzip -c /tmp/pg-dump.sql.gz | psql -h "$PG_HOST" -p "$PG_PORT" \
      -U "$PG_USER" -d "$PG_DB" 2>&1
    rm -f /tmp/pg-dump.sql.gz

    echo "[restore_backup] Postgres restore complete."
  else
    echo "[restore_backup] No Postgres dump found at ${SRC}/db/pg-dump.sql.gz — skipping."
  fi
else
  echo "[restore_backup] PGPASSWORD not set — skipping Postgres restore."
fi

# ─── 3. Qdrant restore ────────────────────────────────────────────────
echo "[restore_backup] Step 3/3: Qdrant restore..."

if rclone ls "${SRC}/db/wiki-snapshot.snapshot" &>/dev/null; then
  echo "[restore_backup] Downloading Qdrant snapshot from S3..."
  rclone copy "${SRC}/db/wiki-snapshot.snapshot" /tmp/ --s3-no-check-bucket

  if [ -f /tmp/wiki-snapshot.snapshot ] && [ -s /tmp/wiki-snapshot.snapshot ]; then
    echo "[restore_backup] Uploading snapshot to Qdrant..."
    curl -s -X POST \
      -F "snapshot=@/tmp/wiki-snapshot.snapshot" \
      "http://qdrant:6333/collections/wiki/snapshots/upload?priority=snapshot" || {
      echo "[restore_backup] Qdrant snapshot upload failed — the collection may not exist yet."
    }
    rm -f /tmp/wiki-snapshot.snapshot
    echo "[restore_backup] Qdrant restore complete."
  fi
else
  echo "[restore_backup] No Qdrant snapshot found at ${SRC}/db/wiki-snapshot.snapshot — skipping."
fi

echo "[restore_backup] Full restore complete."
