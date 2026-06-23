use sql_forge::sql_forge;
use sqlx::PgPool;

use crate::db::types::ChannelUsageStats;

// ---------------------------------------------------------------------------
// Usage stats query — channel-level token usage
// ---------------------------------------------------------------------------

/// Get token usage stats aggregated per channel.
/// Shows model, input_tokens, cached_tokens, output_tokens for each channel.
pub async fn get_channel_usage_stats(pool: &PgPool) -> anyhow::Result<Vec<ChannelUsageStats>> {
    let rows: Vec<ChannelUsageStats> = sql_forge!(
        ChannelUsageStats,
        r#"
        SELECT
            c.id AS channel_id,
            c.name AS channel_name,
            COALESCE(NULLIF(t.model, ''), '(not set)') AS model,
            COALESCE(SUM(t.input_tokens), 0)::bigint AS total_input_tokens,
            COALESCE(SUM(t.cached_tokens), 0)::bigint AS total_cached_tokens,
            COALESCE(SUM(t.output_tokens), 0)::bigint AS total_output_tokens,
            COUNT(t.id)::bigint AS total_threads,
            COALESCE(SUM(t.duration_ms), 0)::bigint AS total_duration_ms
        FROM channels c
        LEFT JOIN threads t ON t.channel_id = c.id
        WHERE t.status IN ('completed', 'failed', 'interrupted', 'skipped')
        GROUP BY c.id, c.name, t.model
        ORDER BY c.name ASC, t.model ASC
        "#,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows)
}
