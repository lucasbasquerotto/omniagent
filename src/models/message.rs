use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

/// Status of a message in its lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MessageStatus {
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "processing")]
    Processing,
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "failed")]
    Failed,
}

impl fmt::Display for MessageStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MessageStatus::Pending => write!(f, "pending"),
            MessageStatus::Processing => write!(f, "processing"),
            MessageStatus::Completed => write!(f, "completed"),
            MessageStatus::Failed => write!(f, "failed"),
        }
    }
}

impl sqlx::Type<sqlx::Postgres> for MessageStatus {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        <String as sqlx::Type<sqlx::Postgres>>::type_info()
    }
}

impl sqlx::Decode<'_, sqlx::Postgres> for MessageStatus {
    fn decode(
        value: sqlx::postgres::PgValueRef<'_>,
    ) -> Result<Self, Box<dyn std::error::Error + 'static + Send + Sync>> {
        let s = <String as sqlx::Decode<sqlx::Postgres>>::decode(value)?;
        match s.to_lowercase().as_str() {
            "pending" => Ok(MessageStatus::Pending),
            "processing" => Ok(MessageStatus::Processing),
            "completed" => Ok(MessageStatus::Completed),
            "failed" => Ok(MessageStatus::Failed),
            _ => Err(format!("invalid message status: {}", s).into()),
        }
    }
}

impl sqlx::Encode<'_, sqlx::Postgres> for MessageStatus {
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> sqlx::encode::IsNull {
        <String as sqlx::Encode<sqlx::Postgres>>::encode(self.to_string(), buf)
    }
}

/// A full message record as stored in the database.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Message {
    pub id: Uuid,
    pub channel_id: Uuid,
    pub role: String,
    pub content: String,
    pub status: MessageStatus,
    pub thread_id: Uuid,
    pub thread_sequence: i32,
    pub external_id: Option<String>,
    pub metadata: serde_json::Value,
    pub embedding: Option<String>,
    pub summary_text: Option<String>,
    pub is_summary: bool,
    pub created_at: DateTime<Utc>,
}

/// Payload for creating a new message (without server-assigned fields).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageNew {
    pub channel_id: Uuid,
    pub role: String,
    pub content: String,
    pub status: MessageStatus,
    pub thread_id: Uuid,
    pub thread_sequence: i32,
    pub external_id: Option<String>,
    pub metadata: serde_json::Value,
    pub embedding: Option<String>,
    pub summary_text: Option<String>,
    pub is_summary: bool,
}
