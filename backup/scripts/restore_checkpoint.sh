#!/bin/bash
set -euo pipefail

# Restore checkpoint: sync from S3_BUCKET/S3_PATH/checkpoint/YYYYMMDD/ to /opt/data/
# Usage: restore_checkpoint 20260616

: "${S3_BUCKET:?S3_BUCKET not set}"
: "${S3_PATH:?S3_PATH not set}"

export RCLONE_CONFIG=${RCLONE_CONFIG:-/etc/rclone/rclone.conf}

if [ $# -lt 1 ]; then
    echo "Usage: restore_checkpoint YYYYMMDD"
    echo "Example: restore_checkpoint 20260616"
    exit 1
fi

DATE_SUFFIX="$1"
# Validate format
if ! echo "$DATE_SUFFIX" | grep -qE '^[0-9]{8}$'; then
    echo "Error: Date must be in YYYYMMDD format (got: $DATE_SUFFIX)"
    exit 1
fi

SRC="${S3_BUCKET}/${S3_PATH}/checkpoint/${DATE_SUFFIX}/"

echo "[restore_checkpoint] Starting: s3-backup:${SRC} → /opt/data/"

rclone sync \
    "s3-backup:${SRC}" \
    /opt/data/ \
    --create-empty-src-dirs \
    --verbose

echo "[restore_checkpoint] Complete."
