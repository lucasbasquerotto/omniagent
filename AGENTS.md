# OmniAgent — AGENTS.md

## Guidelines

### SQL Queries: Always use sql_forge!()
**Every SQL query MUST use `sql_forge!()`.** No raw `sqlx::query`, `sqlx::query_as`, or `sqlx::query_scalar` except where sqlx's compile-time macros cannot handle the type:

- **pgvector `<=>` operator** — The `vector` type from pgvector is not in sqlx's hardcoded compile-time type registry. This affects `sqlx::query_as!` and `sql_forge!()` equally. Use `sqlx::query_as::<_, DbStruct>()` (runtime) with a comment explaining why.

DDL (CREATE TABLE, ALTER TABLE, CREATE INDEX) works with `sql_forge!("SQL")` in execute-only form (no struct, no params) — it wraps `sqlx::query()` at runtime.

Dynamic SQL (variable column sets) should be decomposed into individual static `sql_forge!()` UPDATEs per field rather than building SQL strings at runtime.

**Type discipline:** Always match Rust types to the actual PostgreSQL column types:
- `INT4` (INTEGER) → `i32` or `Option<i32>`
- `INT8` (BIGINT) → `i64` or `Option<i64>`
- `TEXT` / `VARCHAR` → `String` or `Option<String>`
- `TIMESTAMPTZ` → `chrono::DateTime<Utc>` or `Option<...>`
- `JSONB` → `serde_json::Value` or `String` (with `.to_string()` for jsonb casts)

Never cast in Rust (`as i32`, `as i64`) when sql_forge can infer the correct type — use the right sql_forge scalar type instead.

### Column Aliases: No sqlx Proprietary Suffixes
**NEVER use sqlx-proprietary `?` / `!` suffixes in column aliases** (`AS "column?"`, `AS "column!"`).

These suffixes are handled by `sqlx::query_as!` (compile-time) but **NOT** by `sqlx::FromRow` (runtime). At runtime, `FromRow` looks for column names matching the Rust field names exactly, so `AS "created_at!"` produces a column named `created_at!` in the result — which `FromRow` can't find when looking for `created_at`.

**Correct approach:**
- Use `Option<T>` in the DB struct for expression columns with unknown nullability (COALESCE, TO_CHAR, etc.)
- Strip the suffix from the SQL alias so the column name matches the Rust field
- Convert to the domain type in `TryFrom` with `.unwrap_or_default()` / `as_deref().unwrap_or("")` (safe since COALESCE guarantees non-null)

The `.sqlx/` offline cache must be regenerated whenever the DB schema changes:
```bash
cargo sqlx prepare -- --bin omniagent
```

### Error Handling
- Use `anyhow::Result` for fallible functions
- Use `tracing` (info/warn/error) for logging, never `println!`

### Module Structure
- `src/db/types.rs` — All DB queries
- `src/agent/mod.rs` — Agent loop, message processing
- `src/mcp/tools/` — Individual tool implementations
- `src/prompt_builder.rs` — System prompt assembly
- `src/context_builder.rs` — Context retrieval assembly

### Tool Development
- Each MCP tool gets its own file in `src/mcp/tools/<name>.rs`
- Register in `default_registry()` in `src/mcp/mod.rs`
- Add to default profile's `allowed_tools` if it should be available by default
- Tool descriptions must include: ACTION PREFIX + USE CASE + NEGATIVE SPACE

### Thread Summaries
- Summaries are stored in the `summaries` table (channel_id, next_thread_id, content, created_at)
- A summary is generated every `2*SUMMARY_WINDOW` completed seq-0 (thread-root) messages per channel
- The window slides by `SUMMARY_WINDOW`, so summaries overlap by half a window
- The last summary for a channel is always included in LLM context as a High-priority block
- Summary generation uses a separate LLM call with `SUMMARY_TOKENS` max tokens (default 4096)
- Old summaries are deleted alongside old messages via the daily cleanup task
- Config env vars: `SUMMARY_WINDOW` (default 10), `SUMMARY_TOKENS` (default 4096), `DELETE_AFTER_DAYS` (default 30)

### Research Efficiency
- Research tasks follow the RESEARCH_WORKFLOW: read input → search_messages → search_wiki → batch fetch → write output → verify
- Target 2-4 tool-calling rounds max for research tasks
- Batch all HTTP fetches into a single round — never fetch one URL at a time
- Verify output by reading the written file back after writing
- Full documentation at `wiki/Reference/Research.md`
