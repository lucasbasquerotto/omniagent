use anyhow::{Context, Result};
use std::net::SocketAddr;

#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub qdrant_url: String,
    pub host: String,
    pub port: u16,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let database_url = std::env::var("DATABASE_URL")
            .context("DATABASE_URL must be set")?;
        let qdrant_url = std::env::var("QDRANT_URL")
            .unwrap_or_else(|_| "http://localhost:6333".to_string());
        let host = std::env::var("HOST")
            .unwrap_or_else(|_| "0.0.0.0".to_string());
        let port = std::env::var("PORT")
            .unwrap_or_else(|_| "8080".to_string())
            .parse()
            .context("PORT must be a valid number")?;

        Ok(Self {
            database_url,
            qdrant_url,
            host,
            port,
        })
    }

    pub fn socket_addr(&self) -> Result<SocketAddr> {
        let addr = format!("{}:{}", self.host, self.port);
        addr.parse().context("Invalid socket address")
    }
}
