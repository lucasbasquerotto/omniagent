FROM alpine:latest

RUN apk add --no-cache rclone dcron bash tini postgresql-client curl

# Copy scripts
COPY scripts/backup.sh /usr/bin/backup
COPY scripts/checkpoint.sh /usr/bin/checkpoint
COPY scripts/restore_backup.sh /usr/bin/restore_backup
COPY scripts/restore_checkpoint.sh /usr/bin/restore_checkpoint
COPY scripts/entrypoint.sh /entrypoint.sh

RUN chmod +x /usr/bin/backup /usr/bin/checkpoint /usr/bin/restore_backup /usr/bin/restore_checkpoint /entrypoint.sh

# Create cron log directory
RUN mkdir -p /var/log

# The crontab is generated dynamically by entrypoint.sh based on CRON_BACKUP and CRON_CHECKPOINT
ENTRYPOINT ["/sbin/tini", "--", "/entrypoint.sh"]
