# OmniAgent

Next-generation agent system built with Rust, PostgreSQL + pgvector, and MCP tool support.

## Features

### 🧠 Context Builder & Grounding
- **Priority-ranked prompt assembly** (`ContextBuilder`) — NeverTrim (system, MEMORY.md) → High (thread messages) → Normal (tool defs) → Low (retrieved content)
- **Token budgeting** — per-block character caps, lowest-priority blocks dropped when over budget
- **Grounding policy** — embedded in every system prompt: prefer evidence, state uncertainty, cite references
- **Evidence metadata** — `messages.metadata` captures context diagnostics (`context.selected_message_ids`, `block_counts`, `dropped_blocks`, `total_chars`) and grounding flags

### 🔍 Hybrid Retrieval
- **4-tier retrieval** controlled by profile `retrieval_aggressiveness` (0-3):
  - Level 1: ILIKE text search in messages + wiki text search (walkdir)
  - Level 2+: pgvector semantic search (`<=>` cosine similarity on message embeddings) + Qdrant vector search on wiki content
- **Query classifier** — heuristic (Greeting/Command/FollowUp/Factual/ExternalQuery) gates whether retrieval runs
- Re-ranking with recency and same-thread boosts

### 💾 Memory Promotion
- **3 MCP tools** (`promote_to_memory`, `list_memories`, `review_memories`)
- YAML frontmatter with `confidence`, `source_message_ids`, `source_tool_outputs`, `created_at`, `expires_at`, `last_verified_at`
- 30-day default expiry with review workflow

### 🔌 MCP External Servers
- **stdio transport** — spawn subprocesses, JSON-RPC 2.0 over stdin/stdout
- **HTTP transport** — connect to remote MCP servers via HTTP POST
- **Circuit breaker** — automatic disable after N consecutive failures
- **Dynamic tool registry** — external tools auto-merge with built-in tools at startup
- Configured via `MCP_SERVERS_CONFIG` env var or `<data_dir>/config/mcp-servers.json`

### Requirements

- Docker & Docker Compose
- An LLM API key (OpenCode Go, OpenAI, or Anthropic)

### Setup

1. Clone the repo:
   ```bash
   git clone https://github.com/nexuslbs/omniagent.git
   cd omniagent
   ```

2. Copy the environment template and configure:
   ```bash
   cp .env.example .env
   ```
   Edit `.env` and set at minimum:
   - `LLM_API_KEY` — your LLM provider API key
   - `DATABASE_URL` — PostgreSQL connection string (default: `postgres://omniagent:***@postgres:5432/omniagent`)

3. Start the stack:
   ```bash
   docker compose up -d
   ```

This starts:
- **PostgreSQL 16 + pgvector** — message storage with vector embeddings
- **Qdrant** — vector similarity search (optional, for semantic search)
- **OmniAgent** — the agent itself, on port 8080

### Verify

```bash
curl http://localhost:8080/health
# → ok
```

## Channels

Channels represent communication endpoints. Each channel has its own state, profile, and model configuration. The agent processes messages **sequentially within a channel** but **in parallel across channels**.

### Creating a Channel

```sql
INSERT INTO channels (name, platform, external_id, cause, current_profile)
VALUES ('my-channel', 'api', 'my-channel-1', 'user', 'default');
```

Each channel can set a custom profile, provider, and model:
```sql
UPDATE channels SET current_profile = 'research', current_provider = 'anthropic', current_model = 'claude-sonnet-4' WHERE id = 1;
```

### Cron Channel

Every OmniAgent instance has a default cron channel (platform='cron', name='cron-default') created automatically. This channel is used as the fallback destination for cron jobs and kanban tasks that don't specify a channel. It is marked as `readonly=true` to prevent accidental deletion.

### Readonly Channels

Channels can be marked as `readonly` (e.g., the default cron channel) to protect them from deletion:
```sql
ALTER TABLE channels ADD COLUMN readonly BOOLEAN NOT NULL DEFAULT false;
```

## Profiles

Profiles bundle model configuration, provider, and allowed tools. A `default` profile is created on first startup.

Profile fields:
- **provider** — LLM provider (e.g., `opencode-go`, `openai`, `anthropic`)
- **model** — LLM model name (e.g., `deepseek-v4-flash`)
- **allowed_tools** — which MCP tools the agent can use

### Creating a Profile

```sql
INSERT INTO profiles (name, provider, model, allowed_tools)
VALUES (
  'research',
  'anthropic',
  'claude-sonnet-4',
  '["filesystem_read", "filesystem_write", "fetch", "search_messages", "search_wiki"]'
);
```

### Profile vs Channel Priority

The effective model and provider are resolved as:
1. **Message** `profile` (highest) — set per-message for cron/kanban tasks
2. **Channel** `current_profile` / `current_model` / `current_provider`
3. **Profile** `model` / `provider`
4. Environment defaults
5. Built-in fallbacks

If neither the channel nor the profile specifies a model, the prompt will fail with an error.

## Execution Model

### Sequential Per Channel, Parallel Across Channels

The agent runs a **supervisor loop** that:
1. Lists all channels from the database
2. Spawns a dedicated `channel_handler` task for each channel that isn't already running
3. Each `channel_handler` independently polls its channel for pending messages
4. Within a channel, messages are processed one at a time (FIFO order)
5. Across channels, processing happens in parallel

```
┌─────────────────────────────────────────────────┐
│  Supervisor Loop (every 5 sec)                   │
│                                                   │
│  ├── Channel A ── handler ── msg₁ ── msg₂ ── ... │
│  ├── Channel B ── handler ── msg₁ ── msg₂ ── ... │
│  ├── Channel C ── handler ── msg₁ ── msg₂ ── ... │
│  └── cron/kanban ── handler ── msg₁ ── msg₂ ... │
└─────────────────────────────────────────────────┘
```

### Message Lifecycle

```
User inserts a message (status = pending)
  │
  ▼
Agent picks it up, marks as processing
  │
  ├─ LLM responds with text → saved as msg_type='message'
  ├─ LLM includes reasoning → saved as msg_type='reasoning' (separate row)
  └─ LLM calls tools → tool executed, result fed back, loop continues
  │
  ▼
Prompt marked as completed, processing_time_ms set
```

### Profile Resolution at Message Time

When a message is created (seq-0), the `provider` and `model` fields are **stamped** on the message using this resolution chain:
1. **Channel** `current_provider` / `current_model` (highest priority)
2. **Profile** `provider` / `model` (if set in the profile file)
3. **Environment variables** `LLM_PROVIDER` / `LLM_MODEL`
4. **Built-in defaults** `opencode-go` / `deepseek-v4-flash`

This happens at creation time for:
- **User messages**: provider/model are stamped when the message is inserted
- **Cron jobs**: provider/model are resolved and stamped by the cron scheduler
- **Kanban tasks**: when a task is moved to 'ready' status, provider/model are resolved and stamped

### Provider/Model Validation at Execution Time

When the agent picks up a pending message for processing, it **validates** the stamped fields before calling the LLM:

1. `profile` must be non-empty → fails with `msg_type='error'`, `msg_subtype='no-profile'`
2. Profile must exist in the registry → fails with `msg_subtype='invalid-profile'`
3. `provider` must be set and non-empty → fails with `msg_subtype='no-provider'`
4. `model` must be set and non-empty → fails with `msg_subtype='no-model'`

If validation fails, an error message is inserted into the thread and the original message is marked as `failed`. The agent uses **only** the stamped values — no fallback chain is run during execution.

For **cron jobs**: profile comes from the cron job's `profile` field, or the channel's `current_profile` if NULL
For **kanban tasks**: profile comes from the task's `profile` field, or the channel's `current_profile` if NULL
For **user messages**: profile comes from the channel's `current_profile` at message creation time

## Cron Jobs

Cron jobs are scheduled tasks that execute on a recurring schedule. Each job can target a specific channel and profile.

### Creating a Cron Job

```sql
-- Via MCP tool (recommended)
-- Use the create_cron_job tool with optional channel_id and profile params

-- Or directly in SQL:
INSERT INTO cron_jobs (id, name, display_name, schedule, prompt, channel_id, profile)
VALUES ('cron_abc123', 'hourly-report', 'Hourly Report', '0 0 * * * * *', 'Generate the hourly report', 1, 'research');
```

### Fields

| Field | Description |
|-------|-------------|
| `channel_id` | Channel to fire in (NULL = default cron channel) |
| `profile` | Profile to use (NULL = channel's current_profile) |
| `schedule` | 7-field quartz cron expression |
| `prompt` | The message content to execute |
| `enabled` | Whether the job is active |

### Scheduler

The cron scheduler runs as a background tokio task, polling every 30 seconds. When a job is due:
1. The job is atomically claimed (with stale-lock detection after 10 minutes)
2. The target channel is resolved (job's channel_id or default cron channel)
3. The profile is resolved (job's profile or channel's current_profile)
4. A pending seq-0 system message is inserted with `msg_type='cron'`
5. The message's `profile` field is set to the resolved profile
6. The job's timestamps are updated

Concurrency is enforced at the DB level: `UPDATE ... WHERE NOT running` ensures only one scheduler instance fires each job.

## Kanban Tasks

Kanban tasks provide a structured workflow. Tasks can be assigned to channels and when moved to 'ready' status, they trigger execution.

### Creating a Kanban Task

```sql
-- Via MCP tool (recommended)
-- Use the create_kanban_task tool with optional channel_id and profile params

-- Or directly in SQL:
INSERT INTO kanban_tasks (id, title, body, status, channel_id, profile)
VALUES ('task_abc123', 'Research topic', 'Find latest papers on...', 'todo', 1, 'research');
```

### Task Lifecycle

1. Task is created (typically in `backlog` or `todo` status)
2. Task is updated to `ready` status
3. The system automatically creates a pending seq-0 message in the task's channel
4. The agent picks up the message and processes it
5. After completion, the task can be moved to `review` or `done`

### Statuses

| Status | Description |
|--------|-------------|
| `backlog` | Not yet prioritized |
| `todo` | Ready to be worked on |
| `ready` | Triggers execution (creates a pending message) |
| `running` | Currently being executed |
| `review` | Waiting for review/approval |
| `done` | Completed |
| `blocked` | Blocked by something |

### Channel and Profile Assignment

Each kanban task can specify:
- `channel_id`: Which channel to execute in (NULL = default cron channel)
- `profile`: Which profile to use (NULL = channel's current_profile at execution time)

When a task is updated to `ready` status, the system:
1. Resolves the target channel (task's channel_id or default cron channel)
2. Resolves the profile (task's profile or channel's current_profile)
3. Creates a pending seq-0 message with `msg_type='kanban'` and `msg_subtype=<task_id>`
4. The agent processes the message like any other pending message

## Stopping and Resuming

### `POST /stop/{channel_id}`

Stop processing for a specific channel:

```bash
curl -X POST http://localhost:8080/stop/1
```

This will:
1. Mark all **pending** and **processing** messages in the channel as `skipped`
2. Record the stop in the `channel_stops` table
3. Cancel the channel's processing task
4. The supervisor will not respawn a handler for this channel until resumed

### `GET /resume/{channel_id}`

Resume processing for a stopped channel:

```bash
curl http://localhost:8080/resume/1
```

This will:
1. Delete the stop record from `channel_stops`
2. The supervisor will detect the channel is no longer stopped
3. A fresh handler will be spawned in idle state
4. New pending messages will be processed immediately

### Behavior

- Messages created **before** the stop are skipped
- Messages created **after** the stop remain pending and will be processed when resumed
- The channel handler restarts fresh — no state is carried over from before the stop
- If the executor was in the middle of processing a message when `/stop` was called, that message is also marked as skipped

## Configuration Reference

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `OMNI_DATA_DIR` | `/opt/data` | Profile and tools directory |
| `DATABASE_URL` | `postgres://omniagent:***@postgres:5432/omniagent` | PostgreSQL connection string |
| `QDRANT_URL` | `http://localhost:6333` | Qdrant endpoint |
| `LLM_API_KEY` | — | API key for LLM provider |
| `LLM_PROVIDER` | `opencode-go` | Provider: opencode-go, openai, anthropic |
| `LLM_MODEL` | `deepseek-v4-flash` | Default LLM model |
| `LLM_BASE_URL` | *per provider* | API endpoint URL |
| `MAX_TOKENS` | `4096` | Max response tokens |
| `TEMPERATURE` | `0.7` | Sampling temperature |
| `MAX_ITERATIONS` | `60` | Max agent turns per thread |
| `HOST` | `0.0.0.0` | HTTP bind address |
| `PORT` | `8080` | HTTP port |
| `DELETE_AFTER_DAYS` | `30` | Message retention period |
| `MCP_SERVERS_CONFIG` | — | External MCP servers config file path |
| `VECTORIZE_MESSAGES` | `false` | Enable message embedding generation |
| `VECTORIZE_WIKI` | `false` | Enable wiki embedding generation |

## API Endpoints

### `GET /health`

Health check. Returns `ok` with status 200.

### `POST /stop/{channel_id}`

Stop processing for a channel. All pending and processing messages are marked as `skipped`.

### `POST /resume/{channel_id}`

Resume processing for a stopped channel.

### `GET /prompt/{channel_name}`

Show the system prompt that would be used for a given channel (for debugging).

## Sending Messages

Messages are inserted directly into the database. The agent polls for `pending` messages every second.

```sql
INSERT INTO messages (channel_id, thread_id, thread_sequence, role, content, status, msg_type, iteration_count, profile)
VALUES (1, 1, 0, 'user', 'Your prompt here', 'pending', 'message', 0, 'default');
```

### Message Fields

| Field | Description |
|-------|-------------|
| `profile` | Profile to use for processing (overrides channel's current_profile) |
| `msg_type` | Type: `message`, `cron`, `kanban`, `tool`, `tool_result`, `reasoning`, `summary`, `plan` |
| `msg_subtype` | For kanban/cron: stores the task/job ID |

## Backup Container

The stack includes a standalone **backup** container for S3 data durability. It is agent-agnostic — does not require the agent to be running, making it suitable for setup on a new machine before the agent starts.

### Architecture

```yaml
services:
  backup:
    build: ./backup
    env_file: backup.env          # NOT git-versioned
    volumes:
      - ./data:/opt/data:rw
```

### Commands

Run inside the container (`docker compose exec backup <command>`):

| Command | Description |
|---------|-------------|
| `backup` | Syncs `/opt/data/` to `S3_BUCKET/S3_PATH/data/` |
| `checkpoint` | Syncs `/opt/data/` to `S3_BUCKET/S3_PATH/checkpoint/YYYYMMDD/` |
| `restore_backup` | Syncs from `S3_BUCKET/S3_PATH/data/` to `/opt/data/` |
| `restore_checkpoint YYYYMMDD` | Syncs from `S3_BUCKET/S3_PATH/checkpoint/YYYYMMDD/` to `/opt/data/` |

### Configuration (`backup.env`)

| Variable | Example | Description |
|----------|---------|-------------|
| `S3_ENDPOINT` | `https://s3.us-east-005.backblazeb2.com` | S3-compatible endpoint |
| `S3_REGION` | `us-east-005` | S3 region |
| `S3_BUCKET` | `my-bucket` | S3 bucket name |
| `S3_PATH` | `omni` | Path prefix within the bucket |
| `S3_ACCESS_KEY` | — | S3 access key ID |
| `S3_SECRET_KEY` | — | S3 secret access key |
| `CRON_BACKUP` | `"0 5 * * *"` | Backup schedule (empty = disabled) |
| `CRON_CHECKPOINT` | `"0 3 * * 0"` | Checkpoint schedule (empty = disabled) |

Both backup and checkpoint use `rclone sync` with rclone v1.74+.

## Data Directory Structure

Persistent data lives under `OMNI_DATA_DIR` (default `/opt/data`):

```
$OMNI_DATA_DIR/
  profiles/
    default/
      memories/         # Memory files (MEMORY.md, SOUL.md)
      skills/           # Reusable skills
      wiki/             # Wiki reference content
        Memory/
          Promoted/     # Promoted long-term memories
  config/
    mcp-servers.json    # External MCP server config (optional)
  tools/                # MCP tool definitions
```

## Architecture Diagram

```
┌──────────────┐     ┌────────────────┐     ┌────────────┐
│   Messages   │────>│   OmniAgent    │────>│    LLM     │
│ (PostgreSQL) │     │    (Rust)      │     │  Provider  │
└──────────────┘     │                │     └────────────┘
                     │  ┌──────────┐  │
┌──────────────┐     │  │   MCP    │  │
│   Qdrant     │<────│  │  Tools   │  │
│  (Vectors)   │     │  └──────────┘  │
└──────────────┘     └────────────────┘
```

Messages flow: **PG → Agent → LLM → (tool calls loop) → PG**

## Docker Compose

### Production

```yaml
services:
  postgres:
    image: pgvector/pgvector:pg16
    expose: ["5432"]

  qdrant:
    image: qdrant/qdrant:v1.18.2
    expose: ["6333"]

  omniagent:
    build: .
    depends_on: [postgres, qdrant]
    env_file: .env
    expose: ["8080"]
    volumes:
      - ./.env:/app/.env:ro
```

### Development

For local development outside Docker:
```bash
# Run postgres + qdrant, then:
cargo run
```

The binary reads `.env` automatically via `dotenvy`.

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| Messages stay `pending` | Channel stopped or agent not running | Check `GET /health`, resume channel |
| LLM call fails | API key missing or invalid | Check `LLM_API_KEY` in `.env` |
| Processing stuck at `processing` | Container restarted mid-call | On restart, pending/processing messages are marked as skipped |
| No model configured | Profile + channel both lack model | Set `current_model` on channel or `model` on profile |
| Tools returning errors | Path outside data directory | Ensure file paths are under `OMNI_DATA_DIR` |

## Internal Docs

For detailed internal architecture, see [AGENTS.md](AGENTS.md).

## Testing

### Test Environment Setup

The system uses PostgreSQL for all state. Test data is injected via direct SQL:

```bash
# Insert a test thread with cause message
docker compose exec postgres psql -U omniagent -d omniagent
```

### Thread Lifecycle Tests

| Test | Setup | Expected |
|------|-------|----------|
| **Single channel, all causes** | 3 threads (user/cron/kanban) → same channel → set pending | Processed **sequentially** (one after another). All complete |
| **Different channels (parallelism)** | 3 threads in 3 different channels → set pending | Processed at the **same second** — each channel handler runs independently |
| **Stop/Resume** | Start a thread → `curl stop/<id>` → verify `skipped` → `resume` → new message | Stopped thread = `skipped`. New thread after resume picks up immediately |
| **Empty provider** | Thread with `provider=''` | **failed** with clear error: "provider is not set" |
| **Empty model** | Thread with `model=''` | **failed** with clear error: "model is not set" |
| **Nonexistent profile** | Thread with `profile='nonexistent'` | Falls back to **default** profile (intentional feature) |

### Verification Commands

```bash
# Watch processing in real-time
docker compose logs -f omniagent | grep -E "Processing|completed|summary|failed"

# Query thread state
docker compose exec postgres psql -U omniagent -d omniagent -c "
SELECT t.id, t.status, t.cause, c.name as ch,
       (SELECT count(*) FROM messages m WHERE m.thread_id = t.id) as msg_count
FROM threads t JOIN channels c ON t.channel_id = c.id
WHERE t.channel_id = <ch_id> ORDER BY t.id;"

# Stop/Resume API
curl http://localhost:8080/stop/<channel_id>
curl http://localhost:8080/resume/<channel_id>"

