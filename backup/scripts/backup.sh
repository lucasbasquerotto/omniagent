#!/bin/bash
set -euo pipefail

# Backup: sync /opt/data/ to S3_BUCKET/S3_PATH/data/
: "${S3_BUCKET:?S3_BUCKET not set}"
: "${S3_PATH:?S3_PATH not set}"

export RCLONE_CONFIG=${RCLONE_CONFIG:-/etc/rclone/rclone.conf}

echo "[backup] Starting: /opt/data/ → s3-backup:${S3_BUCKET}/${S3_PATH}/data/"

rclone sync /opt/data/ \
    "s3-backup:${S3_BUCKET}/${S3_PATH}/data/" \
    --create-empty-src-dirs \
    --verbose

echo "[backup] Complete."
