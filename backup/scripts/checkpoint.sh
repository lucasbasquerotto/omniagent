#!/bin/bash
set -euo pipefail

# Checkpoint: sync /opt/data/ to S3_BUCKET/S3_PATH/checkpoint/YYYYMMDD/
: "${S3_BUCKET:?S3_BUCKET not set}"
: "${S3_PATH:?S3_PATH not set}"

export RCLONE_CONFIG=${RCLONE_CONFIG:-/etc/rclone/rclone.conf}

DATE_SUFFIX=$(date +%Y%m%d)
DEST="${S3_BUCKET}/${S3_PATH}/checkpoint/${DATE_SUFFIX}/"

echo "[checkpoint] Starting: /opt/data/ → s3-backup:${DEST}"

rclone sync /opt/data/ \
    "s3-backup:${DEST}" \
    --create-empty-src-dirs \
    --verbose

echo "[checkpoint] Complete."
