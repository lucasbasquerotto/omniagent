use anyhow::Result;
use sqlx::PgPool;

pub async fn run(pool: &PgPool) -> Result<()> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS channels (
            id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            name        TEXT NOT NULL,
            platform    TEXT NOT NULL,
            external_id TEXT NOT NULL,
            metadata    JSONB DEFAULT '{}',
            created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            UNIQUE(platform, external_id)
        );
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS messages (
            id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            channel_id  UUID NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
            role        TEXT NOT NULL,
            content     TEXT NOT NULL,
            embedding   vector(1536),
            external_id TEXT,
            metadata    JSONB DEFAULT '{}',
            created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            UNIQUE(channel_id, external_id)
        );
        "#,
    )
    .execute(pool)
    .await?;

    // Enable the pgvector extension if not already enabled
    sqlx::query("CREATE EXTENSION IF NOT EXISTS vector;")
        .execute(pool)
        .await?;

    Ok(())
}
