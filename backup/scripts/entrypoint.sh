#!/bin/bash
set -euo pipefail

# Entrypoint for the backup container.
# Generates rclone config from S3 env vars and starts crond.

# Validate required vars
: "${S3_ACCESS_KEY:?S3_ACCESS_KEY not set}"
: "${S3_SECRET_KEY:?S3_SECRET_KEY not set}"
: "${S3_ENDPOINT:?S3_ENDPOINT not set}"
: "${S3_REGION:?S3_REGION not set}"
: "${S3_BUCKET:?S3_BUCKET not set}"
: "${S3_PATH:?S3_PATH not set}"

# Write rclone config for the named remote "s3-backup"
mkdir -p /etc/rclone
cat > /etc/rclone/rclone.conf <<EOF
[s3-backup]
type = s3
provider = Other
access_key_id = ${S3_ACCESS_KEY}
secret_access_key = ${S3_SECRET_KEY}
endpoint = ${S3_ENDPOINT}
region = ${S3_REGION}
EOF

chmod 600 /etc/rclone/rclone.conf

echo "[entrypoint] rclone config written for remote 's3-backup'"

# Generate crontab if CRON_BACKUP or CRON_CHECKPOINT are set
CRONTAB_FILE=/tmp/crontab
rm -f "$CRONTAB_FILE"

if [ -n "${CRON_BACKUP:-}" ]; then
    echo "${CRON_BACKUP} RCLONE_CONFIG=/etc/rclone/rclone.conf /usr/bin/backup >> /var/log/backup.log 2>&1" >> "$CRONTAB_FILE"
    echo "[entrypoint] Scheduled backup: ${CRON_BACKUP}"
fi

if [ -n "${CRON_CHECKPOINT:-}" ]; then
    echo "${CRON_CHECKPOINT} RCLONE_CONFIG=/etc/rclone/rclone.conf /usr/bin/checkpoint >> /var/log/checkpoint.log 2>&1" >> "$CRONTAB_FILE"
    echo "[entrypoint] Scheduled checkpoint: ${CRON_CHECKPOINT}"
fi

if [ -f "$CRONTAB_FILE" ]; then
    crontab "$CRONTAB_FILE"
    echo "[entrypoint] Crontab installed."
fi

echo "[entrypoint] Backup container ready."

# Run crond in foreground with verbose logging to stdout
crond -f -l 2 -L /dev/stdout
