#!/bin/bash
# Backup: sync /opt/data/ + Postgres dump + Qdrant snapshot to S3
set -euo pipefail

: "${S3_BUCKET:?S3_BUCKET not set}"
: "${S3_PATH:?S3_PATH not set}"

export RCLONE_CONFIG=${RCLONE_CONFIG:-/etc/rclone/rclone.conf}

DEST="s3-backup:${S3_BUCKET}/${S3_PATH}"

echo "[backup] Starting full backup..."

# ─── 1. File data ─────────────────────────────────────────────────────
echo "[backup] Step 1/3: File data → ${DEST}/data/"
rclone sync /opt/data/ "${DEST}/data/" --create-empty-src-dirs --s3-no-check-bucket --verbose

# ─── 2. Postgres dump ─────────────────────────────────────────────────
echo "[backup] Step 2/3: Postgres dump..."
PG_HOST="${PGHOST:-postgres}"
PG_PORT="${PGPORT:-5432}"
PG_USER="${PGUSER:-omniagent}"
PG_DB="${PGDATABASE:-omniagent}"

if [ -n "${PGPASSWORD:-}" ]; then
  export PGPASSWORD
  pg_dump -h "$PG_HOST" -p "$PG_PORT" -U "$PG_USER" -d "$PG_DB" \
    --no-owner --no-acl 2>/dev/null | gzip > /tmp/pg-dump.sql.gz
  rclone copy /tmp/pg-dump.sql.gz "${DEST}/db/" --s3-no-check-bucket
  rm -f /tmp/pg-dump.sql.gz
  echo "[backup] Postgres dump uploaded."
else
  echo "[backup] PGPASSWORD not set — skipping Postgres backup."
fi

# ─── 3. Qdrant snapshot ───────────────────────────────────────────────
echo "[backup] Step 3/3: Qdrant wiki snapshot..."
SNAPSHOT_RESPONSE=$(wget -qO- --post-data="" http://qdrant:6333/collections/wiki/snapshots 2>&1) || {
  echo "[backup] Qdrant snapshot creation failed — skipping."
}
if [ -n "$SNAPSHOT_RESPONSE" ]; then
  SNAPSHOT_NAME=$(echo "$SNAPSHOT_RESPONSE" | sed 's/.*"name":"//' | sed 's/".*//')
  echo "[backup] Snapshot: $SNAPSHOT_NAME"
  wget -qO /tmp/wiki-snapshot.snapshot "http://qdrant:6333/collections/wiki/snapshots/$SNAPSHOT_NAME"
  rclone copy /tmp/wiki-snapshot.snapshot "${DEST}/db/" --s3-no-check-bucket
  rm -f /tmp/wiki-snapshot.snapshot
  echo "[backup] Qdrant snapshot uploaded."
fi

echo "[backup] Full backup complete."
