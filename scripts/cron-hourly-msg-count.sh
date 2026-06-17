#!/bin/bash
# Hourly message count — counts user/system messages (seq 0) from the last hour
# Output delivered verbatim as the cron result

HOST="${PGHOST:-host.docker.internal}"
PORT="${PGPORT:-5432}"
USER="${PGUSER:-omniagent}"
DB="${PGDATABASE:-omniagent}"

COUNT=$(psql -h "$HOST" -p "$PORT" -U "$USER" -d "$DB" -t -A -c "
  SELECT COUNT(*)
  FROM messages
  WHERE role IN ('user', 'system')
    AND thread_sequence = 0
    AND created_at >= NOW() - INTERVAL '1 hour';
" 2>/dev/null)

if [ -z "$COUNT" ]; then COUNT=0; fi

echo "Hourly message report — ${COUNT} user/system messages in the last hour."
