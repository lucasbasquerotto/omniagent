// Database schema definitions.
//
// This module documents the database table schemas for the OmniAgent system.
// Migrations are run via raw SQL in the migrations module.
//
// ── channels ──────────────────────────────────────────────────────────────
//
// Stores communication channels (e.g., Telegram group/channel, cron jobs).
//
//  id          UUID PRIMARY KEY DEFAULT gen_random_uuid()
//  name        TEXT NOT NULL            -- e.g. "user-lucas", "cron-daily-backup"
//  platform    TEXT NOT NULL            -- e.g. "telegram", "cron"
//  external_id TEXT NOT NULL            -- e.g. Telegram chat ID
//  cause       TEXT NOT NULL            -- 'user' or 'cron'
//  metadata    JSONB DEFAULT '{}'       -- arbitrary metadata
//  created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
//  updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
//
//  UNIQUE(platform, external_id)
//
// ── messages ──────────────────────────────────────────────────────────────
//
// Stores messages received across channels, including agent replies and tool
// calls. Messages are grouped into threads for conversation tracking.
//
//  id               UUID PRIMARY KEY DEFAULT gen_random_uuid()
//  channel_id       UUID NOT NULL REFERENCES channels(id) ON DELETE CASCADE
//  role             TEXT NOT NULL       -- 'user', 'agent', 'system', 'tool'
//  content          TEXT NOT NULL       -- message body
//  status           TEXT NOT NULL DEFAULT 'pending'
//                                      -- 'pending', 'processing', 'completed', 'failed'
//  thread_id        UUID NOT NULL      -- groups related messages
//  thread_sequence  INT NOT NULL       -- order within thread
//  external_id      TEXT               -- e.g. Telegram message ID
//  metadata         JSONB DEFAULT '{}' -- arbitrary metadata
//  embedding        vector(1536)       -- pgvector embedding vector
//  summary_text     TEXT               -- cached summary of the message
//  is_summary       BOOL NOT NULL DEFAULT false
//  created_at       TIMESTAMPTZ NOT NULL DEFAULT NOW()
//
//  UNIQUE(channel_id, external_id)
//  UNIQUE(thread_id, thread_sequence)
//
// ── Indexes ───────────────────────────────────────────────────────────────
//
//  idx_messages_channel_status  ON messages(channel_id, status, created_at)
//  idx_messages_thread          ON messages(thread_id, thread_sequence)
//
// ── Extension ─────────────────────────────────────────────────────────────
//
//  pgvector (CREATE EXTENSION vector) — provides vector(1536) type for
//  embedding storage and similarity search.
//
