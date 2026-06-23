use sql_forge::sql_forge;
use sqlx::PgPool;

use crate::db::types::SubscriptionDb;

// ---------------------------------------------------------------------------
// Subscription CRUD functions
// ---------------------------------------------------------------------------

/// Add a subscription: a channel subscriber (platform+resource) will receive
/// summaries from the given channel.
pub async fn add_subscription(
    pool: &PgPool,
    channel_id: i64,
    subscriber_platform: &str,
    subscriber_resource: &str,
) -> anyhow::Result<i64> {
    #[derive(Debug, sqlx::FromRow)]
    struct SubId {
        id: i64,
    }
    let row: SubId = sql_forge!(
        SubId,
        r#"
        INSERT INTO channel_subscriptions (channel_id, subscriber_platform, subscriber_resource)
        VALUES (:channel_id, :subscriber_platform, :subscriber_resource)
        ON CONFLICT (channel_id, subscriber_platform, subscriber_resource)
        DO UPDATE SET created_at = NOW()
        RETURNING id
        "#,
        ( :channel_id = channel_id, :subscriber_platform = subscriber_platform, :subscriber_resource = subscriber_resource )
    )
    .fetch_one(pool)
    .await?;
    Ok(row.id)
}

/// Remove a subscription. Returns true if a row was actually deleted.
pub async fn remove_subscription(
    pool: &PgPool,
    channel_id: i64,
    subscriber_platform: &str,
    subscriber_resource: &str,
) -> anyhow::Result<bool> {
    let result = sql_forge!(
        "DELETE FROM channel_subscriptions WHERE channel_id = :channel_id AND subscriber_platform = :subscriber_platform AND subscriber_resource = :subscriber_resource",
        ( :channel_id = channel_id, :subscriber_platform = subscriber_platform, :subscriber_resource = subscriber_resource )
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Get all subscribers for a given channel (the channel whose summaries are
/// being subscribed to).
pub async fn get_subscribers_for_channel(
    pool: &PgPool,
    channel_id: i64,
) -> anyhow::Result<Vec<SubscriptionDb>> {
    let rows: Vec<SubscriptionDb> = sql_forge!(
        SubscriptionDb,
        r#"
        SELECT
            id, channel_id, subscriber_platform, subscriber_resource,
            COALESCE(TO_CHAR(created_at, 'YYYY-MM-DD"T"HH24' || CHR(58) || 'MI' || CHR(58) || 'SS.US"Z"'), '') AS "created_at"
        FROM channel_subscriptions
        WHERE channel_id = :channel_id
        ORDER BY id ASC
        "#,
        ( :channel_id = channel_id )
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Get all subscriptions for a given subscriber (what channels does this
/// subscriber receive summaries from).
pub async fn get_subscriptions_for_subscriber(
    pool: &PgPool,
    subscriber_platform: &str,
    subscriber_resource: &str,
) -> anyhow::Result<Vec<SubscriptionDb>> {
    let rows: Vec<SubscriptionDb> = sql_forge!(
        SubscriptionDb,
        r#"
        SELECT
            id, channel_id, subscriber_platform, subscriber_resource,
            COALESCE(TO_CHAR(created_at, 'YYYY-MM-DD"T"HH24' || CHR(58) || 'MI' || CHR(58) || 'SS.US"Z"'), '') AS "created_at"
        FROM channel_subscriptions
        WHERE subscriber_platform = :subscriber_platform AND subscriber_resource = :subscriber_resource
        ORDER BY id ASC
        "#,
        ( :subscriber_platform = subscriber_platform, :subscriber_resource = subscriber_resource )
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}
