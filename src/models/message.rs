use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: Uuid,
    pub channel_id: Uuid,
    pub role: String,
    pub content: String,
    pub external_id: Option<String>,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
}
