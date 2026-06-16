use anyhow::Result;
use sqlx::PgPool;

pub async fn run(pool: &PgPool) -> Result<()> {
    // Enable pgvector extension FIRST
    sqlx::query("CREATE EXTENSION IF NOT EXISTS vector;")
        .execute(pool)
        .await?;

    // Create channels table
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS channels (
            id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            name        TEXT NOT NULL,
            platform    TEXT NOT NULL,
            external_id TEXT NOT NULL,
            cause       TEXT NOT NULL,
            metadata    JSONB DEFAULT '{}',
            created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            UNIQUE(platform, external_id)
        );
        "#,
    )
    .execute(pool)
    .await?;

    // Create messages table
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS messages (
            id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            channel_id      UUID NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
            role            TEXT NOT NULL,
            content         TEXT NOT NULL,
            status          TEXT NOT NULL DEFAULT 'pending',
            thread_id       UUID NOT NULL,
            thread_sequence INT NOT NULL,
            external_id     TEXT,
            metadata        JSONB DEFAULT '{}',
            embedding       vector(1536),
            summary_text    TEXT,
            is_summary      BOOL NOT NULL DEFAULT false,
            created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            UNIQUE(channel_id, external_id),
            UNIQUE(thread_id, thread_sequence)
        );
        "#,
    )
    .execute(pool)
    .await?;

    // Create indexes
    sqlx::query(
        r#"
        CREATE INDEX IF NOT EXISTS idx_messages_channel_status
            ON messages(channel_id, status, created_at);
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE INDEX IF NOT EXISTS idx_messages_thread
            ON messages(thread_id, thread_sequence);
        "#,
    )
    .execute(pool)
    .await?;

    Ok(())
}
