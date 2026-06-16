use anyhow::Result;
use tracing_subscriber::EnvFilter;

mod agent;
mod config;
mod db;
mod models;
mod platform;

#[tokio::main]
async fn main() -> Result<()> {
    // Load .env file if present
    dotenvy::dotenv().ok();

    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    tracing::info!("OmniAgent starting...");

    // Load base configuration
    let cfg = config::Config::from_env()?;
    tracing::info!("Configuration loaded: {:?}", cfg);

    // Connect to PostgreSQL
    let pool = db::connect(&cfg.database_url).await?;
    tracing::info!("Connected to PostgreSQL");

    // Run migrations
    db::migrations::run(&pool).await?;
    tracing::info!("Database migrations completed successfully");

    // Create agent config from environment
    let agent_cfg = agent::AgentConfig::from_env()?;
    tracing::info!(
        "Agent config — model: {}, provider: {}, max_tokens: {}, temperature: {}",
        agent_cfg.llm_model,
        agent_cfg.llm_provider,
        agent_cfg.max_tokens,
        agent_cfg.temperature,
    );

    // Build the agent
    let agent = agent::Agent::new(pool.clone(), agent_cfg);

    // Create platform registry and register built-in platforms
    let mut registry = platform::PlatformRegistry::new();
    registry.register(Box::new(platform::TelegramPlatform::new()));

    // Start all platform listener tasks
    let _platform_handles = registry.start_all(pool.clone());

    // Spawn the agent loop as a concurrent task
    let agent_handle = tokio::spawn(async move {
        if let Err(e) = agent.run().await {
            tracing::error!("Agent loop exited with error: {:?}", e);
        }
    });

    tracing::info!("OmniAgent is ready! Waiting for messages...");

    // Graceful shutdown on Ctrl+C
    tokio::select! {
        _ = agent_handle => {
            tracing::info!("Agent loop finished");
        }
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Received Ctrl+C, shutting down...");
        }
    }

    tracing::info!("OmniAgent shutdown complete");
    Ok(())
}
