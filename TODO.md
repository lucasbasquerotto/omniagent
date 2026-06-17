# TODO — Improve Context Grounding, Memory, and MCP Extensibility

## 1) Context Builder (Selective, Ranked Prompt Assembly)

- [ ] Create a `ContextBuilder` pipeline before each LLM call in `agent::process_message`.
- [ ] Assemble prompt context from ordered blocks:
  - [ ] System/profile instructions
  - [ ] `MEMORY.md` (always include, hard cap <= 5000 chars)
  - [ ] Recent thread messages (recency window)
  - [ ] Last user messages (pinned)
  - [ ] Retrieved past messages (relevance-ranked)
  - [ ] Retrieved wiki snippets (relevance-ranked)
  - [ ] Allowed tool definitions only
- [ ] Add token budgeting per block (reserve output tokens, trim lowest-priority blocks first).
- [ ] Persist context assembly metadata in `messages.metadata` (selected message IDs, wiki files, token counts).

## 2) Retrieval Strategy (Past Messages + Wiki)

- [ ] Implement hybrid retrieval for historical context:
  - [ ] Semantic retrieval (pgvector / embeddings)
  - [ ] Keyword fallback (ILIKE / lexical)
- [ ] Add re-ranking step favoring:
  - [ ] Recency
  - [ ] Same thread/channel
  - [ ] User-confirmed facts
- [ ] Add retrieval guardrails:
  - [ ] Max snippets per source type
  - [ ] Per-snippet char/token cap
  - [ ] Dedup by semantic similarity

## 3) Hallucination Reduction / Grounding Policy

- [ ] Update system/profile prompt policy:
  - [ ] Prefer retrieved evidence over prior assumptions
  - [ ] If uncertain, explicitly state uncertainty
  - [ ] For factual/project-specific claims, provide grounding references
- [ ] Add internal evidence structure in metadata for each final answer:
  - [ ] `evidence.messages[]` (message IDs)
  - [ ] `evidence.wiki[]` (file paths/sections)
  - [ ] `evidence.tools[]` (tool call IDs)
- [ ] Add low-confidence fallback behavior:
  - [ ] Ask clarifying question, or
  - [ ] Trigger retrieval/tool call before answering
- [ ] Add contradiction check between drafted answer and retrieved evidence.

## 4) Memory Model (Short-Term vs Long-Term)

- [ ] Keep full raw history in `messages` table (all roles/types) as source of truth.
- [ ] Introduce explicit long-term memory promotion workflow to wiki:
  - [ ] Promote only validated/repeatedly useful facts
  - [ ] Store provenance (source message IDs / tool outputs)
  - [ ] Store confidence and `last_verified_at`
- [ ] Add review/expiry workflow for long-term memory entries.
- [ ] Keep `MEMORY.md` user-authored and always-included (size-capped).

## 5) “Remember to Retrieve” Behavior

- [ ] Add question classifier (fast heuristic/model): decide when retrieval is required.
- [ ] Auto-trigger `search_messages` / `search_wiki` for factual or repo-specific queries.
- [ ] Add profile-level knobs:
  - [ ] `auto_retrieval_enabled`
  - [ ] `retrieval_aggressiveness`
  - [ ] `grounding_required`

## 6) MCP Runtime Hardening (Current Built-in Tools)

- [ ] Enforce strict JSON Schema validation for tool inputs.
- [ ] Add per-tool timeout/retry policy and error taxonomy.
- [ ] Add idempotency and side-effect classification (`read_only`, `mutating`, `external_network`).
- [ ] Require confirmation gate for high-risk mutating tools.
- [ ] Improve observability:
  - [ ] Persist tool latency, success/failure class, retry count
  - [ ] Link tool call/result records with stable IDs

## 7) MCP Extensibility (Add Tools Without Binary Release)

- [ ] Design external MCP server integration:
  - [ ] Transport support: `stdio` first, HTTP/SSE next
  - [ ] Capability negotiation + tool discovery
- [ ] Add dynamic tool registry layer:
  - [ ] Merge built-in + external tools at runtime
  - [ ] Profile-level allowlist enforcement across both
- [ ] Add secure secret handling for external tool auth.
- [ ] Add health checks / circuit breaker per external MCP server.

## 8) Data/Schema & Telemetry Enhancements

- [ ] Extend `messages.metadata` schema conventions for:
  - [ ] context selection diagnostics
  - [ ] evidence references
  - [ ] confidence score
- [ ] Add tables (or JSON schema) for:
  - [ ] feedback signals (explicit/implicit)
  - [ ] wiki memory provenance and verification status
- [ ] Add periodic metrics jobs:
  - [ ] Groundedness rate
  - [ ] Retrieval hit rate
  - [ ] Tool success rate
  - [ ] Hallucination proxy metrics (user corrections/re-asks)

## 9) Evaluation Loop (Continuous Improvement)

- [ ] Build eval dataset from real conversations + expected outcomes.
- [ ] Add regression suite for profile/model/prompt changes.
- [ ] Track quality/cost/latency per profile and model.
- [ ] Block prompt/profile rollouts on eval regressions.

## 10) Rollout Plan

- [ ] Phase 1: Context builder + grounding policy + metadata evidence logging.
- [ ] Phase 2: Hybrid retrieval + auto-retrieval trigger + contradiction checks.
- [ ] Phase 3: Memory promotion workflow + provenance + review cycle.
- [ ] Phase 4: MCP external servers + dynamic tool registry.
- [ ] Phase 5: Full eval/feedback-driven optimization.

## 11) Documentation Updates

- [ ] Update `AGENTS.md` with:
  - [ ] Context assembly rules
  - [ ] Grounding/citation policy
  - [ ] Memory promotion criteria
  - [ ] MCP extension model (built-in vs external)
- [ ] Add operator runbook for tuning retrieval and grounding settings.

