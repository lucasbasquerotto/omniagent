use anyhow::Result;
use pgvector::Vector;
use sqlx::PgPool;
use std::str::FromStr;
use uuid::Uuid;

use crate::models::{Channel, Message, MessageNew, MessageStatus};

/// Find the oldest pending messages for a channel, ordered by created_at.
pub async fn find_pending_messages(
    pool: &PgPool,
    channel_id: Uuid,
) -> Result<Vec<Message>> {
    let rows = sqlx::query_as::<_, Message>(
        r#"
        SELECT
            id,
            channel_id,
            role,
            content,
            status,
            thread_id,
            thread_sequence,
            external_id,
            metadata,
            embedding::text AS embedding,
            summary_text,
            is_summary,
            created_at
        FROM messages
        WHERE channel_id = $1 AND status = 'pending'
        ORDER BY created_at ASC
        "#,
    )
    .bind(channel_id)
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

/// Insert a new message into the database.
pub async fn create_message(pool: &PgPool, msg: &MessageNew) -> Result<Message> {
    let embedding_sql: Option<Vector> = match &msg.embedding {
        Some(s) => Some(Vector::from_str(s)?),
        None => None,
    };

    let row = sqlx::query_as::<_, Message>(
        r#"
        INSERT INTO messages (
            channel_id, role, content, status,
            thread_id, thread_sequence, external_id,
            metadata, embedding, summary_text, is_summary
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
        RETURNING
            id,
            channel_id,
            role,
            content,
            status,
            thread_id,
            thread_sequence,
            external_id,
            metadata,
            embedding::text AS embedding,
            summary_text,
            is_summary,
            created_at
        "#,
    )
    .bind(msg.channel_id)
    .bind(&msg.role)
    .bind(&msg.content)
    .bind(&msg.status)
    .bind(msg.thread_id)
    .bind(msg.thread_sequence)
    .bind(&msg.external_id)
    .bind(&msg.metadata)
    .bind(embedding_sql)
    .bind(&msg.summary_text)
    .bind(msg.is_summary)
    .fetch_one(pool)
    .await?;

    Ok(row)
}

/// Update the status of a message by its id.
pub async fn update_message_status(
    pool: &PgPool,
    id: Uuid,
    status: &MessageStatus,
) -> Result<()> {
    sqlx::query("UPDATE messages SET status = $1 WHERE id = $2")
        .bind(status)
        .bind(id)
        .execute(pool)
        .await?;

    Ok(())
}

/// Find a channel by its name.
pub async fn get_channel_by_name(pool: &PgPool, name: &str) -> Result<Option<Channel>> {
    let row = sqlx::query_as::<_, Channel>(
        r#"
        SELECT
            id,
            name,
            platform,
            external_id,
            cause,
            metadata,
            created_at,
            updated_at
        FROM channels
        WHERE name = $1
        "#,
    )
    .bind(name)
    .fetch_optional(pool)
    .await?;

    Ok(row)
}

/// Find all channels.
pub async fn find_all_channels(pool: &PgPool) -> Result<Vec<Channel>> {
    let rows = sqlx::query_as::<_, Channel>(
        r#"
        SELECT
            id,
            name,
            platform,
            external_id,
            cause,
            metadata,
            created_at,
            updated_at
        FROM channels
        ORDER BY name ASC
        "#,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

/// Find messages with status='processing' created before the given timestamp.
pub async fn find_processing_older_than(
    pool: &PgPool,
    before: chrono::DateTime<chrono::Utc>,
) -> Result<Vec<Message>> {
    let rows = sqlx::query_as::<_, Message>(
        r#"
        SELECT
            id,
            channel_id,
            role,
            content,
            status,
            thread_id,
            thread_sequence,
            external_id,
            metadata,
            embedding::text AS embedding,
            summary_text,
            is_summary,
            created_at
        FROM messages
        WHERE status = 'processing' AND created_at < $1
        ORDER BY created_at ASC
        "#,
    )
    .bind(before)
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

/// Create a new channel, or return the existing one if a channel with the
/// same (platform, external_id) already exists.
pub async fn create_channel(
    pool: &PgPool,
    name: &str,
    platform: &str,
    external_id: &str,
    cause: &str,
) -> Result<Channel> {
    let row = sqlx::query_as::<_, Channel>(
        r#"
        INSERT INTO channels (name, platform, external_id, cause)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (platform, external_id)
        DO UPDATE SET
            updated_at = NOW()
        RETURNING
            id,
            name,
            platform,
            external_id,
            cause,
            metadata,
            created_at,
            updated_at
        "#,
    )
    .bind(name)
    .bind(platform)
    .bind(external_id)
    .bind(cause)
    .fetch_one(pool)
    .await?;

    Ok(row)
}
