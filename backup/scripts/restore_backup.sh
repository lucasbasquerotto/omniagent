#!/bin/bash
set -euo pipefail

# Restore backup: sync from S3_BUCKET/S3_PATH/data/ to /opt/data/
: "${S3_BUCKET:?S3_BUCKET not set}"
: "${S3_PATH:?S3_PATH not set}"

export RCLONE_CONFIG=${RCLONE_CONFIG:-/etc/rclone/rclone.conf}

echo "[restore_backup] Starting: s3-backup:${S3_BUCKET}/${S3_PATH}/data/ → /opt/data/"

rclone sync \
    "s3-backup:${S3_BUCKET}/${S3_PATH}/data/" \
    /opt/data/ \
    --create-empty-src-dirs \
    --verbose

echo "[restore_backup] Complete."
