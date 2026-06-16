use anyhow::Result;
use tracing_subscriber::EnvFilter;

mod config;
mod db;
mod models;

#[tokio::main]
async fn main() -> Result<()> {
    // Load .env file if present
    dotenvy::dotenv().ok();

    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    tracing::info!("OmniAgent starting...");

    // Load configuration
    let cfg = config::Config::from_env()?;
    tracing::info!("Configuration loaded: {:?}", cfg);

    // Connect to PostgreSQL
    let pool = db::connect(&cfg.database_url).await?;
    tracing::info!("Connected to PostgreSQL");

    // Run migrations
    db::migrations::run(&pool).await?;
    tracing::info!("Database migrations completed successfully");

    tracing::info!("OmniAgent is ready to serve!");
    Ok(())
}
