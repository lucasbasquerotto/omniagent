# OmniAgent Optimization Skill

## Overview

This skill documents the iterative optimization process for OmniAgent — a Rust-based autonomous agent. The goal is to improve:
- **Token efficiency** (reduce prompt tokens, increase cache ratio)
- **Loop efficiency** (fewer LLM turns, fewer duplicate tool calls)
- **Memory/wiki retrieval** (use existing knowledge before fetching external data)
- **Summary generation** (ensure summary always runs)
- **Result quality** (comprehensive, well-structured output)

## Baseline Metrics (Pre-Optimization)

From thread 635 (PostgreSQL vector extensions research):
| Metric | Value |
|--------|-------|
| Total messages | 81 |
| LLM iterations | 8 |
| Fetch calls | 36 (4 duplicates) |
| Total time | 96s |
| Prompt tokens | 410,874 |
| Completion tokens | 21,036 |
| Cached tokens | 342,144 (83%) |
| Summary generated | ❌ No |
| Memory/wiki accessed | ❌ No |
| Reasoning tokens tracked | 0 (not tracked) |

## Optimization Techniques

### 1. Tool-Level Dedup Cache

**Problem**: LLM fetches the same URL multiple times (e.g., `pg_embedding` GitHub API fetched 3x). The LLM has no memory of previous tool calls in the current session.

**Solution**: Add an in-memory HashMap in `process_message()` that caches tool results by `(tool_name, serialized_args)`. Before executing a tool, check if we already have the result. Return cached result without calling the tool.

Implementation in `src/agent/mod.rs`:
```rust
use std::collections::HashMap;

// Add fn prune_old_tool_results after:
struct ToolCache {
    cache: HashMap<(String, String), String>, // (tool_name, args_json) -> result
}

impl ToolCache {
    fn get(&self, name: &str, args: &str) -> Option<&str> {
        self.cache.get(&(name.to_string(), args.to_string())).map(String::as_str)
    }
    fn set(&mut self, name: &str, args: &str, result: &str) {
        self.cache.insert((name.to_string(), args.to_string()), result.to_string());
    }
}
```

### 2. Context Pruning Before Summary

**Problem**: The summary LLM call clones ALL messages including full tool results. With 80+ messages this can exceed the context window silently.

**Solution**: Strip tool results from the summary prompt, keeping only user+agent messages. The summary only needs to know what was accomplished, not the raw data.

```rust
// Before summary generation, strip oversized tool results
let mut summary_msgs: Vec<ChatMessage> = messages.iter()
    .filter(|m| m.role != "tool") // strip all tool results
    .cloned()
    .collect();
```

### 3. Aggressive History Pruning

**Problem**: All 8 iterations' tool results accumulate, making each subsequent LLM call more expensive.

**Solution**: Reduce `TOOL_RESULT_HISTORY_BUDGET` from 80K to 40K chars. This forces pruning earlier, keeping only the most recent turn's results.

### 4. Fetch Result Dedup via First-URL Strategy

**Problem**: For each extension, the LLM fetches: GitHub HTML page, raw README, GitHub API repo data, GitHub API releases, GitHub API tags, crates.io API, PyPI... Many of these are redundant (README contains the info).

**Solution**: Add system prompt guidance to fetch the README first (best single source), and only fetch additional APIs if the README doesn't contain the needed info.

### 5. Summary Generation Reliability

**Problem**: The `let _ =` pattern on line 1000 of `agent/mod.rs` swallows `create_message` errors silently. Combined with possible LLM failures on oversized context, the summary silently disappears.

**Solution**: 
- Prune tool results before summary (see #2)
- Log summary creation success/failure explicitly
- Track summary as a separate metric

### 6. Memory/Wiki Retrieval Activation

**Problem**: The agent doesn't search past messages or wiki before researching. With 80+ existing messages about vector search, it could start from existing knowledge.

**Solution**: 
- Ensure `auto_retrieval_enabled` is enabled in profile
- Add system prompt hint: "Before fetching external data, use search_messages and search_wiki to check if relevant information already exists in past conversations or the wiki."
- Set `retrieval_aggressiveness` to 2+ for research tasks

### 7. Token Tracking for Reasoning

**Problem**: `reasoning_tokens` is always 0/null. The DeepSeek model returns reasoning tokens but they may be in different fields.

**Solution**: Check the LLM response structure for `reasoning_tokens` field. The provider may use different field names (e.g., DeepSeek uses `reasoning_tokens` at the top level in some responses).

## Test Protocol

### Before/After Comparison

For each optimization, run a research task with:
1. Different topic each time (no cheating by reusing topics)
2. Same tools available
3. Same max iterations
4. Measure: token usage, time, LLM turns, duplicates, summary presence

### Research Topics (9 iterations)

Each iteration uses a different research topic to ensure generalization:
1. PostgreSQL vector extensions (pgvector, pgvecto.rs, pg_embedding, pg_vectorize) — DONE as baseline
2. Rust async runtimes (Tokio vs async-std vs smol)
3. Container orchestration (Kubernetes vs Nomad vs Docker Swarm)
4. Python web frameworks (FastAPI vs Django vs Flask vs Starlette)
5. Database technologies (SQLite vs DuckDB vs ClickHouse)
6. JavaScript runtimes (Node.js vs Deno vs Bun)
7. Message queue systems (NATS vs RabbitMQ vs Kafka vs Pulsar)
8. CSS framework comparison (Tailwind vs Bootstrap vs Bulma vs Material UI)
9. Machine learning frameworks (PyTorch vs TensorFlow vs JAX)

## Verification Queries

After each test run, verify with:

```sql
-- Full analysis
SELECT msg_type, msg_subtype, COUNT(*) as count, SUM(processing_time_ms) as total_time
FROM messages WHERE thread_id = <thread_id>
GROUP BY msg_type, msg_subtype ORDER BY msg_type, msg_subtype;

-- Duplicate fetch URLs
SELECT content as url, COUNT(*) as times
FROM messages WHERE thread_id = <thread_id> AND msg_type = 'tool' AND msg_subtype = 'fetch'
GROUP BY content HAVING COUNT(*) > 1 ORDER BY times DESC;

-- Token usage
SELECT 
  SUM((token_usage->>'prompt_tokens')::int) as prompt,
  SUM((token_usage->>'completion_tokens')::int) as completion,
  SUM(COALESCE((token_usage->>'cached_tokens')::int, 0)) as cached,
  MIN(created_at) as started, MAX(created_at) as finished
FROM messages WHERE thread_id = <thread_id>;

-- Summary check
SELECT id, msg_type FROM messages WHERE thread_id = <thread_id> AND msg_type = 'summary';

-- Memory/wiki access
SELECT msg_subtype FROM messages WHERE thread_id = <thread_id> 
  AND msg_subtype IN ('search_messages', 'search_wiki');

-- Timeline
SELECT id, msg_type, msg_subtype, created_at::timestamp::text
FROM messages WHERE thread_id = <thread_id> AND 
  (msg_type IN ('message', 'reasoning', 'summary') OR role = 'user')
ORDER BY id;
```
