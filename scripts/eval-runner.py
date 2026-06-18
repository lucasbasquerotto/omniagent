#!/usr/bin/env python3
"""
OmniAgent Evaluation Runner

Runs test prompts through the agent and records results for regression
comparison. Supports:
  - Running individual eval cases by name
  - Full regression suite
  - Comparing results against baselines

Usage:
  # Run full eval suite
  python3 scripts/eval-runner.py --db-url postgres://... --channel 1

  # Run specific cases
  python3 scripts/eval-runner.py --db-url postgres://... --channel 1 --case "basic_qna"

  # List available cases
  python3 scripts/eval-runner.py --list-cases

  # Compare with baseline
  python3 scripts/eval-runner.py --db-url postgres://... --channel 1 --compare
"""

import argparse
import json
import os
import subprocess
import sys
import time
from datetime import datetime, timezone
from typing import Optional

# ---------------------------------------------------------------------------
# Eval Cases
# ---------------------------------------------------------------------------

EVAL_CASES = {
    "basic_qna": {
        "prompt": "What is the capital of France?",
        "tags": ["basic", "knowledge"],
        "expected_grounded": False,
        "max_tokens": 500,
    },
    "file_read": {
        "prompt": "Read the file README.md and summarize it in one sentence.",
        "tags": ["tool", "filesystem"],
        "expected_grounded": False,
        "max_tokens": 1000,
    },
    "project_question": {
        "prompt": "What is OmniAgent and what technology does it use?",
        "tags": ["basic", "project"],
        "expected_grounded": False,
        "max_tokens": 1000,
    },
    "memory_promotion": {
        "prompt": "Remember that the project uses PostgreSQL with pgvector for embeddings.",
        "tags": ["memory", "tool"],
        "expected_grounded": False,
        "max_tokens": 1000,
    },
    "complex_task": {
        "prompt": "List all files in the project root, count how many .rs files there are, and create a summary.",
        "tags": ["tool", "complex", "filesystem"],
        "expected_grounded": False,
        "max_tokens": 2000,
    },
}


def list_cases():
    """Print available eval cases."""
    print("Available eval cases:\n")
    for name, case in sorted(EVAL_CASES.items()):
        tags_str = ", ".join(case["tags"])
        print(f"  {name:30s} [{tags_str}]")
        print(f"  {'':30s}  {case['prompt'][:60]}...")


# ---------------------------------------------------------------------------
# Agent interaction helpers
# ---------------------------------------------------------------------------


def send_message(
    db_url: str,
    channel_id: int,
    content: str,
    profile: str = "default",
    thread_id: Optional[int] = None,
) -> int:
    """Insert a pending message into the database and return its ID."""
    import psycopg2

    conn = psycopg2.connect(db_url)
    cur = conn.cursor()

    # Get or create thread
    if thread_id is None:
        # Create new thread by inserting root message, then backfill
        cur.execute(
            """
            INSERT INTO messages (channel_id, role, content, status, thread_sequence,
                                  msg_type, iteration_count, profile, metadata)
            VALUES (%s, 'user', %s, 'pending', 0, 'message', 0, %s, '{}'::jsonb)
            RETURNING id
            """,
            (channel_id, content, profile),
        )
        msg_id = cur.fetchone()[0]
        # Backfill thread_id = id
        cur.execute("UPDATE messages SET thread_id = id WHERE id = %s", (msg_id,))
    else:
        cur.execute(
            """
            INSERT INTO messages (channel_id, thread_id, role, content, status, thread_sequence,
                                  msg_type, iteration_count, profile, metadata)
            VALUES (%s, %s, 'user', %s, 'pending', 0, 'message', 0, %s, '{}'::jsonb)
            RETURNING id
            """,
            (channel_id, thread_id, content, profile),
        )
        msg_id = cur.fetchone()[0]

    conn.commit()
    cur.close()
    conn.close()
    return msg_id


def wait_for_response(
    db_url: str, msg_id: int, timeout_secs: int = 60, poll_interval: float = 1.0
) -> dict:
    """Wait for an agent response after a user message."""
    import psycopg2

    start = time.time()
    conn = psycopg2.connect(db_url)
    cur = conn.cursor()

    try:
        while time.time() - start < timeout_secs:
            # Check if original message is completed
            cur.execute(
                "SELECT status, processing_time_ms FROM messages WHERE id = %s",
                (msg_id,),
            )
            row = cur.fetchone()
            if row and row[0] in ("completed", "failed", "skipped", "interrupted"):
                status = row[0]
                processing_ms = row[1]

                # Get the agent response
                cur.execute(
                    """
                    SELECT id, content, msg_type, msg_subtype, processing_time_ms,
                           token_usage, created_at
                    FROM messages
                    WHERE thread_id = %s
                      AND role = 'agent'
                      AND msg_type = 'message'
                    ORDER BY created_at DESC
                    LIMIT 1
                    """,
                    (msg_id,),
                )
                agent_row = cur.fetchone()

                # Get reasoning if present
                cur.execute(
                    """
                    SELECT content FROM messages
                    WHERE thread_id = %s AND role = 'agent' AND msg_type = 'reasoning'
                    ORDER BY created_at DESC LIMIT 1
                    """,
                    (msg_id,),
                )
                reasoning_row = cur.fetchone()

                # Get tool calls
                cur.execute(
                    """
                    SELECT msg_subtype, content FROM messages
                    WHERE thread_id = %s AND role = 'agent' AND msg_type = 'tool_call'
                    ORDER BY created_at ASC
                    """,
                    (msg_id,),
                )
                tool_rows = cur.fetchall()

                return {
                    "message_id": msg_id,
                    "status": status,
                    "processing_time_ms": processing_ms,
                    "agent_response": agent_row[1] if agent_row else None,
                    "agent_response_id": agent_row[0] if agent_row else None,
                    "reasoning": reasoning_row[0] if reasoning_row else None,
                    "tool_calls": [
                        {"tool": r[0], "args": r[1]} for r in tool_rows
                    ],
                    "token_usage": agent_row[5] if agent_row else None,
                    "response_time": time.time() - start,
                }

            time.sleep(poll_interval)

        return {"message_id": msg_id, "status": "timeout", "error": f"Timeout after {timeout_secs}s"}

    finally:
        cur.close()
        conn.close()


# ---------------------------------------------------------------------------
# Results
# ---------------------------------------------------------------------------


class EvalResult:
    def __init__(self, case_name: str, case: dict, response: dict):
        self.case_name = case_name
        self.case = case
        self.response = response
        self.passed = response.get("status") == "completed"
        self.timestamp = datetime.now(timezone.utc).isoformat()

    def to_dict(self) -> dict:
        return {
            "case": self.case_name,
            "prompt": self.case["prompt"],
            "tags": self.case["tags"],
            "timestamp": self.timestamp,
            "passed": self.passed,
            "status": self.response.get("status"),
            "processing_time_ms": self.response.get("processing_time_ms"),
            "agent_response": (self.response.get("agent_response") or "")[:200],
            "has_reasoning": self.response.get("reasoning") is not None,
            "tool_call_count": len(self.response.get("tool_calls", [])),
            "response_time_secs": round(self.response.get("response_time", 0), 2),
        }

    def __str__(self) -> str:
        status_icon = "✅" if self.passed else "❌"
        return (
            f"{status_icon} {self.case_name:30s} "
            f"status={self.response.get('status'):15s} "
            f"tools={len(self.response.get('tool_calls', []))} "
            f"time={self.response.get('processing_time_ms')}ms "
            f"resp={round(self.response.get('response_time', 0), 1)}s"
        )


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------


def save_results(results: list[EvalResult], output_path: str):
    """Save eval results to a JSON file for baseline comparison."""
    data = {
        "timestamp": datetime.now(timezone.utc).isoformat(),
        "results": [r.to_dict() for r in results],
        "summary": {
            "total": len(results),
            "passed": sum(1 for r in results if r.passed),
            "failed": sum(1 for r in results if not r.passed),
        },
    }
    os.makedirs(os.path.dirname(output_path) or ".", exist_ok=True)
    with open(output_path, "w") as f:
        json.dump(data, f, indent=2)
    print(f"\nResults saved to {output_path}")


def load_baseline(path: str) -> dict:
    """Load a previous baseline for comparison."""
    with open(path) as f:
        return json.load(f)


def compare_results(current: list[EvalResult], baseline_path: str):
    """Compare current results against a baseline."""
    if not os.path.exists(baseline_path):
        print(f"No baseline found at {baseline_path}")
        return

    baseline = load_baseline(baseline_path)
    baseline_map = {r["case"]: r for r in baseline["results"]}

    print(f"\n{'=' * 60}")
    print(f"COMPARISON with baseline: {baseline_path}")
    print(f"{'=' * 60}\n")

    regressions = 0
    improvements = 0

    for result in current:
        case_name = result.case_name
        baseline_result = baseline_map.get(case_name)

        if not baseline_result:
            print(f"  🆕 {case_name:30s} (new case, no baseline)")
            continue

        current_time = result.response.get("processing_time_ms") or 0
        baseline_time = baseline_result.get("processing_time_ms") or 0

        if not result.passed and baseline_result.get("passed"):
            print(f"  🔴 {case_name:30s} REGRESSION: was passing, now failing")
            regressions += 1
        elif result.passed and not baseline_result.get("passed"):
            print(f"  🟢 {case_name:30s} IMPROVEMENT: was failing, now passing")
            improvements += 1

        # Time comparison
        if current_time > 0 and baseline_time > 0:
            time_diff = current_time - baseline_time
            pct = (time_diff / baseline_time) * 100
            if abs(pct) > 20:  # Only report significant changes
                direction = "⬆️ slower" if time_diff > 0 else "⬇️ faster"
                print(f"  {direction} {case_name:30s} {baseline_time}ms → {current_time}ms ({pct:+.0f}%)")

    print(f"\nSummary: {regressions} regressions, {improvements} improvements")


def main():
    parser = argparse.ArgumentParser(description="OmniAgent Eval Runner")
    parser.add_argument("--db-url", default=os.environ.get("DATABASE_URL"), help="PostgreSQL connection string")
    parser.add_argument("--channel", type=int, default=1, help="Channel ID to send messages to")
    parser.add_argument("--profile", default="default", help="Profile name")
    parser.add_argument("--case", help="Run a specific eval case by name")
    parser.add_argument("--list-cases", action="store_true", help="List available eval cases")
    parser.add_argument("--output", default="data/eval-results.json", help="Output file for results")
    parser.add_argument("--compare", action="store_true", help="Compare with baseline")
    parser.add_argument("--baseline", default="data/eval-baseline.json", help="Baseline file for comparison")
    parser.add_argument("--save-baseline", action="store_true", help="Save results as new baseline")

    args = parser.parse_args()

    if args.list_cases:
        list_cases()
        return

    if not args.db_url:
        print("Error: DATABASE_URL must be set or --db-url provided")
        sys.exit(1)

    # Determine which cases to run
    if args.case:
        if args.case not in EVAL_CASES:
            print(f"Error: Unknown case '{args.case}'")
            list_cases()
            sys.exit(1)
        cases_to_run = {args.case: EVAL_CASES[args.case]}
    else:
        cases_to_run = EVAL_CASES

    print(f"\n{'=' * 60}")
    print(f"OmniAgent Eval Runner")
    print(f"Channel: {args.channel}, Profile: {args.profile}")
    print(f"Cases: {len(cases_to_run)}")
    print(f"{'=' * 60}\n")

    results = []

    for case_name, case in sorted(cases_to_run.items()):
        print(f"  ▶ Running: {case_name}...", end=" ", flush=True)

        try:
            msg_id = send_message(args.db_url, args.channel, case["prompt"], args.profile)
            response = wait_for_response(args.db_url, msg_id)

            result = EvalResult(case_name, case, response)
            results.append(result)
            print(str(result))

        except Exception as e:
            print(f"  ❌ {case_name}: ERROR - {e}")

        # Brief pause between cases
        time.sleep(1)

    # Summary
    passed = sum(1 for r in results if r.passed)
    failed = sum(1 for r in results if not r.passed)

    print(f"\n{'=' * 60}")
    print(f"RESULTS: {passed}/{len(results)} passed, {failed} failed")
    print(f"{'=' * 60}")

    # Save results
    if args.output:
        save_results(results, args.output)

    # Compare with baseline
    if args.compare:
        baseline_path = args.baseline or args.output.replace("results", "baseline")
        compare_results(results, baseline_path)

    # Save as baseline
    if args.save_baseline:
        baseline_path = args.baseline or "data/eval-baseline.json"
        save_results(results, baseline_path)
        print(f"Baseline saved to {baseline_path}")

    sys.exit(0 if failed == 0 else 1)


if __name__ == "__main__":
    main()
