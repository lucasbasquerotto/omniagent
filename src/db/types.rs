//! DB-focused structs using only primitive types compatible with sql-forge's
//! compile-time validation. Each struct mirrors a domain model but stores
//! complex types (DateTime, JSON, enums) as plain strings. Conversion to
//! domain types is done explicitly in Rust — no SQL type casting.
//!
//! Currently uses raw sqlx queries (runtime-only validation). When the project
//! upgrades sqlx to 0.9, replace `sqlx::query_as` calls with `sql_forge!(...)`
//! macros for compile-time SQL validation.

use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::models::{Channel, ChannelStop, Message, MessageNew, MessageStatus};

// ---------------------------------------------------------------------------
// Message DB struct (for SELECT results)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MessageDb {
    pub id: i64,
    pub channel_id: i64,
    pub role: String,
    pub content: String,
    pub status: String,
    pub thread_id: i64,
    pub thread_sequence: i32,
    pub external_id: Option<String>,
    pub metadata: String,
    pub embedding: Option<String>,
    pub summary_text: Option<String>,
    pub is_summary: bool,
    pub msg_type: String,
    pub msg_subtype: Option<String>,
    pub iteration_count: i32,
    pub profile: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub processing_time_ms: Option<i32>,
    pub token_usage: Option<String>,
    pub created_at: String,
}

impl TryFrom<MessageDb> for Message {
    type Error = anyhow::Error;

    fn try_from(db: MessageDb) -> Result<Self, Self::Error> {
        Ok(Self {
            id: db.id,
            channel_id: db.channel_id,
            role: db.role,
            content: db.content,
            status: db
                .status
                .parse::<MessageStatus>()
                .map_err(|_| anyhow::anyhow!("Invalid status: {}", db.status))?,
            thread_id: db.thread_id,
            thread_sequence: db.thread_sequence,
            external_id: db.external_id,
            metadata: serde_json::from_str(&db.metadata).unwrap_or(serde_json::json!({})),
            embedding: db.embedding,
            summary_text: db.summary_text,
            is_summary: db.is_summary,
            msg_type: db.msg_type,
            msg_subtype: db.msg_subtype,
            iteration_count: db.iteration_count,
            profile: db.profile,
            provider: db.provider,
            model: db.model,
            processing_time_ms: db.processing_time_ms,
            token_usage: db.token_usage.and_then(|v| serde_json::from_str(&v).ok()),
            created_at: db
                .created_at
                .parse::<DateTime<Utc>>()
                .map_err(|e| anyhow::anyhow!("Invalid timestamp '{}': {}", db.created_at, e))?,
        })
    }
}

// ---------------------------------------------------------------------------
// MessageNew DB struct (for INSERT params)
// ---------------------------------------------------------------------------

pub struct MessageNewDb {
    pub channel_id: i64,
    pub role: String,
    pub content: String,
    pub status: String,
    pub thread_id: i64,
    pub thread_sequence: i32,
    pub external_id: Option<String>,
    pub metadata: String,
    pub embedding: Option<String>,
    pub summary_text: Option<String>,
    pub is_summary: bool,
    pub msg_type: String,
    pub msg_subtype: Option<String>,
    pub iteration_count: i32,
    pub profile: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub processing_time_ms: Option<i32>,
    pub token_usage: Option<String>,
}

impl From<&MessageNew> for MessageNewDb {
    fn from(msg: &MessageNew) -> Self {
        Self {
            channel_id: msg.channel_id,
            role: msg.role.clone(),
            content: msg.content.clone(),
            status: msg.status.to_string(),
            thread_id: msg.thread_id,
            thread_sequence: msg.thread_sequence,
            external_id: msg.external_id.clone(),
            metadata: msg.metadata.to_string(),
            embedding: msg.embedding.clone(),
            summary_text: msg.summary_text.clone(),
            is_summary: msg.is_summary,
            msg_type: msg.msg_type.clone(),
            msg_subtype: msg.msg_subtype.clone(),
            iteration_count: msg.iteration_count,
            profile: msg.profile.clone(),
            provider: msg.provider.clone(),
            model: msg.model.clone(),
            processing_time_ms: msg.processing_time_ms,
            token_usage: msg.token_usage.as_ref().map(|v| v.to_string()),
        }
    }
}

// ---------------------------------------------------------------------------
// Channel DB struct (for SELECT results)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ChannelDb {
    pub id: i64,
    pub name: String,
    pub platform: String,
    pub external_id: String,
    pub cause: String,
    pub current_profile: String,
    pub current_model: Option<String>,
    pub current_provider: Option<String>,
    pub metadata: String,
    pub created_at: String,
    pub updated_at: String,
}

impl TryFrom<ChannelDb> for Channel {
    type Error = anyhow::Error;

    fn try_from(db: ChannelDb) -> Result<Self, Self::Error> {
        Ok(Self {
            id: db.id,
            name: db.name,
            platform: db.platform,
            external_id: db.external_id,
            cause: db.cause,
            current_profile: db.current_profile,
            current_model: db.current_model,
            current_provider: db.current_provider,
            metadata: serde_json::from_str(&db.metadata).unwrap_or(serde_json::json!({})),
            created_at: db
                .created_at
                .parse::<DateTime<Utc>>()
                .map_err(|e| anyhow::anyhow!("Invalid timestamp '{}': {}", db.created_at, e))?,
            updated_at: db
                .updated_at
                .parse::<DateTime<Utc>>()
                .map_err(|e| anyhow::anyhow!("Invalid timestamp '{}': {}", db.updated_at, e))?,
        })
    }
}

// ---------------------------------------------------------------------------
// ChannelStop DB struct (for SELECT results)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ChannelStopDb {
    pub id: i64,
    pub channel_id: i64,
    pub stopped_at: String,
}

impl TryFrom<ChannelStopDb> for ChannelStop {
    type Error = anyhow::Error;

    fn try_from(db: ChannelStopDb) -> Result<Self, Self::Error> {
        Ok(Self {
            id: db.id,
            channel_id: db.channel_id,
            stopped_at: db
                .stopped_at
                .parse::<DateTime<Utc>>()
                .map_err(|e| anyhow::anyhow!("Invalid timestamp '{}': {}", db.stopped_at, e))?,
        })
    }
}

// ---------------------------------------------------------------------------
// Query functions using raw sqlx (runtime-only validation)
// Replace with sql_forge!(...) after upgrading sqlx to 0.9
// ---------------------------------------------------------------------------

pub async fn find_pending_messages(pool: &PgPool, channel_id: i64) -> anyhow::Result<Vec<Message>> {
    let rows: Vec<MessageDb> = sqlx::query_as(
        r#"
        SELECT
            id, channel_id, role, content, status,
            thread_id, thread_sequence, external_id,
            metadata::text, embedding, summary_text, is_summary,
            msg_type, msg_subtype, iteration_count,
            profile, provider, model, processing_time_ms, token_usage::text,
            TO_CHAR(created_at, 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS created_at
        FROM messages
        WHERE channel_id = $1 AND status = 'pending'
        ORDER BY created_at ASC
        "#,
    )
    .bind(channel_id)
    .fetch_all(pool)
    .await?;

    rows.into_iter().map(|r| r.try_into()).collect()
}

pub async fn create_message(pool: &PgPool, msg: &MessageNew) -> anyhow::Result<Message> {
    let db = MessageNewDb::from(msg);
    let row: MessageDb = sqlx::query_as(
        r#"
        INSERT INTO messages (
            channel_id, role, content, status,
            thread_id, thread_sequence, external_id,
            metadata, embedding, summary_text, is_summary,
            msg_type, msg_subtype, iteration_count,
            profile, provider, model, processing_time_ms, token_usage
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8::jsonb, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19::jsonb)
        RETURNING
            id, channel_id, role, content, status,
            thread_id, thread_sequence, external_id,
            metadata::text, embedding, summary_text, is_summary,
            msg_type, msg_subtype, iteration_count,
            profile, provider, model, processing_time_ms, token_usage::text,
            TO_CHAR(created_at, 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS created_at
        "#,
    )
    .bind(db.channel_id)
    .bind(&db.role)
    .bind(&db.content)
    .bind(&db.status)
    .bind(db.thread_id)
    .bind(db.thread_sequence)
    .bind(&db.external_id)
    .bind(&db.metadata)
    .bind(&db.embedding)
    .bind(&db.summary_text)
    .bind(db.is_summary)
    .bind(&db.msg_type)
    .bind(&db.msg_subtype)
    .bind(db.iteration_count)
    .bind(&db.profile)
    .bind(&db.provider)
    .bind(&db.model)
    .bind(db.processing_time_ms)
    .bind(&db.token_usage)
    .fetch_one(pool)
    .await?;

    row.try_into()
}

pub async fn update_message_status(
    pool: &PgPool,
    id: i64,
    status: &MessageStatus,
) -> anyhow::Result<()> {
    let status_str = status.to_string();
    sqlx::query("UPDATE messages SET status = $1 WHERE id = $2")
        .bind(&status_str)
        .bind(id)
        .execute(pool)
        .await?;

    Ok(())
}

pub async fn find_all_channels(pool: &PgPool) -> anyhow::Result<Vec<Channel>> {
    let rows: Vec<ChannelDb> = sqlx::query_as(
        r#"
        SELECT
            id, name, platform, external_id, cause,
            current_profile, current_model, current_provider,
            metadata::text, TO_CHAR(created_at, 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS created_at, TO_CHAR(updated_at, 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS updated_at
        FROM channels
        ORDER BY name ASC
        "#,
    )
    .fetch_all(pool)
    .await?;

    rows.into_iter().map(|r| r.try_into()).collect()
}

pub async fn find_processing_older_than(
    pool: &PgPool,
    before: chrono::DateTime<chrono::Utc>,
) -> anyhow::Result<Vec<Message>> {
    let rows: Vec<MessageDb> = sqlx::query_as(
        r#"
        SELECT
            id, channel_id, role, content, status,
            thread_id, thread_sequence, external_id,
            metadata::text, embedding, summary_text, is_summary,
            msg_type, msg_subtype, iteration_count,
            profile, provider, model, processing_time_ms, token_usage::text,
            TO_CHAR(created_at, 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS created_at
        FROM messages
        WHERE status = 'processing' AND created_at < $1
        ORDER BY created_at ASC
        "#,
    )
    .bind(before)
    .fetch_all(pool)
    .await?;

    rows.into_iter().map(|r| r.try_into()).collect()
}

pub async fn count_thread_iterations(pool: &PgPool, thread_id: i64) -> anyhow::Result<i32> {
    let count: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT COUNT(*) FROM messages
        WHERE thread_id = $1
          AND role = 'agent'
          AND msg_type = 'message'
        "#,
    )
    .bind(thread_id)
    .fetch_one(pool)
    .await?;

    Ok(count.unwrap_or(0) as i32)
}

pub async fn skip_pending_messages(pool: &PgPool, channel_id: i64) -> anyhow::Result<u64> {
    let result = sqlx::query(
        "UPDATE messages SET status = 'skipped' WHERE channel_id = $1 AND status = 'pending'",
    )
    .bind(channel_id)
    .execute(pool)
    .await?;

    Ok(result.rows_affected())
}

pub async fn stop_channel(pool: &PgPool, channel_id: i64) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO channel_stops (channel_id)
        VALUES ($1)
        ON CONFLICT (channel_id) DO UPDATE SET stopped_at = NOW()
        "#,
    )
    .bind(channel_id)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn find_stopped_channel(
    pool: &PgPool,
    channel_id: i64,
) -> anyhow::Result<Option<ChannelStop>> {
    let row: Option<ChannelStopDb> = sqlx::query_as(
        r#"
        SELECT id, channel_id, TO_CHAR(stopped_at, 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS stopped_at
        FROM channel_stops
        WHERE channel_id = $1
        "#,
    )
    .bind(channel_id)
    .fetch_optional(pool)
    .await?;

    row.map(|r| r.try_into()).transpose()
}

pub async fn delete_old_messages(
    pool: &PgPool,
    before: chrono::DateTime<chrono::Utc>,
) -> anyhow::Result<u64> {
    let result = sqlx::query("DELETE FROM messages WHERE created_at < $1")
        .bind(before)
        .execute(pool)
        .await?;

    Ok(result.rows_affected())
}

#[expect(dead_code)]
pub async fn get_channel_by_name(pool: &PgPool, name: &str) -> anyhow::Result<Option<Channel>> {
    let row: Option<ChannelDb> = sqlx::query_as(
        r#"
        SELECT
            id, name, platform, external_id, cause,
            current_profile, current_model, current_provider,
            metadata::text, TO_CHAR(created_at, 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS created_at, TO_CHAR(updated_at, 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS updated_at
        FROM channels
        WHERE name = $1
        "#,
    )
    .bind(name)
    .fetch_optional(pool)
    .await?;

    row.map(|r| r.try_into()).transpose()
}

#[expect(dead_code)]
pub async fn create_channel(
    pool: &PgPool,
    name: &str,
    platform: &str,
    external_id: &str,
    cause: &str,
) -> anyhow::Result<Channel> {
    let row: ChannelDb = sqlx::query_as(
        r#"
        INSERT INTO channels (name, platform, external_id, cause)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (platform, external_id)
        DO UPDATE SET updated_at = NOW()
        RETURNING
            id, name, platform, external_id, cause,
            current_profile, current_model, current_provider,
            metadata::text, TO_CHAR(created_at, 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS created_at, TO_CHAR(updated_at, 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS updated_at
        "#,
    )
    .bind(name)
    .bind(platform)
    .bind(external_id)
    .bind(cause)
    .fetch_one(pool)
    .await?;

    row.try_into()
}

#[expect(dead_code)]
pub async fn clear_channel_stop(pool: &PgPool, channel_id: i64) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM channel_stops WHERE channel_id = $1")
        .bind(channel_id)
        .execute(pool)
        .await?;

    Ok(())
}

#[expect(dead_code)]
pub async fn find_all_stopped_channels(pool: &PgPool) -> anyhow::Result<Vec<ChannelStop>> {
    let rows: Vec<ChannelStopDb> = sqlx::query_as(
        r#"
        SELECT id, channel_id, TO_CHAR(stopped_at, 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS stopped_at
        FROM channel_stops
        ORDER BY stopped_at DESC
        "#,
    )
    .fetch_all(pool)
    .await?;

    rows.into_iter().map(|r| r.try_into()).collect()
}
