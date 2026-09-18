#!/usr/bin/env python3
"""Tester: parity of the dashboard prompt API (POST /prompt-preview, GET /prompt)
   against a DIRECT call of the configured prompt tool (settings
   prompt_generate_tool, default `prompt__generate`) with identical arguments.

Read-only: it only issues GET/POST preview requests (never a DB write) and
prints the parts of both sides for comparison.
"""
import json
import urllib.request
import urllib.error
import hashlib

BASE = "http://localhost:8080"


def call(method, path, body=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(BASE + path, data=data, method=method,
                                 headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=180) as r:
            return r.status, r.read().decode()
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode()
    except Exception as e:  # noqa: BLE001
        return 0, repr(e)


def md5(s):
    return hashlib.md5(s.encode()).hexdigest()


def find_parts(o, d=0):
    if d > 8:
        return None
    if isinstance(o, dict):
        if "system" in o and ("context" in o or "memory" in o):
            return o
        for v in o.values():
            r = find_parts(v, d + 1)
            if r:
                return r
    elif isinstance(o, list):
        for v in o:
            r = find_parts(v, d + 1)
            if r:
                return r
    return None


# ---------- live plugin registry tool list (what the executor forwards) ----------
st, raw = call("GET", "/mcp/tools")
tools = []
try:
    j = json.loads(raw)
    lst = j if isinstance(j, list) else (j.get("tools") or j.get("data") or j.get("names") or [])
    for t in lst:
        tools.append(t if isinstance(t, str) else (t.get("name") or ""))
except Exception as e:  # noqa: BLE001
    print("TOOLS_ERR", e, raw[:160])
tools = [t for t in tools if t]
print("REGISTRY tools=%d sample=%s" % (len(tools), tools[:3]))

MM = ("You are on a Mattermost messaging platform. Standard markdown formatting is supported: "
      "**bold**, *italic*, `code`, ```code blocks```, [links](url), headings, lists, tables, "
      "blockquotes. Mattermost supports most GFM (GitHub Flavored Markdown).")


def tcall(args, meta=None):
    body = {"name": "prompt__generate", "arguments": args}
    if meta is not None:
        body["meta"] = meta
    st, raw = call("POST", "/mcp/execute", body)
    if st != 200:
        return None, "HTTP%s %s" % (st, raw[:160])
    outer = json.loads(raw)
    parts = find_parts(outer)
    if parts is None and isinstance(outer, dict):
        c = outer.get("content")
        cands = []
        if isinstance(c, str):
            cands.append(c)
        elif isinstance(c, list):
            for ci in c:
                if isinstance(ci, dict) and isinstance(ci.get("text"), str):
                    cands.append(ci["text"])
                elif isinstance(ci, str):
                    cands.append(ci)
        for t in cands:
            try:
                parts = find_parts(json.loads(t))
            except Exception:  # noqa: BLE001
                pass
            if parts:
                break
    if parts is None:
        return None, "SHAPE %s" % raw[:300]
    return parts, None


def mk(platform_key=True, platform="", hint=None, um="<<<prompt>>>", tid=0, ch=""):
    a = {"profile_name": "omni", "platform_hint": hint, "user_message": um,
         "tool_names": tools, "thread_id": tid, "channel_id": ch}
    if platform_key:
        a["platform"] = platform
    return a


def show(tag, p, e):
    if e:
        print("%-24s ERR %s" % (tag, e))
        return None
    s = p.get("system", "")
    print("%-24s sys_len=%-5d md5=%s keys=%s usr=%r"
          % (tag, len(s), md5(s), sorted(p.keys()), (p.get("user") or "")[:24]))
    return p


# ---------- direct tool calls: exactly what the executor/API forwards ----------
direct = {}
for tag, args in [
    ("T default platform=''", mk(True, "", None, "<<<prompt>>>", 0, "")),
    ("T platform omitted", mk(False, "", None, "<<<prompt>>>", 0, "")),
    ("T platform='cli'", mk(True, "cli", None, "<<<prompt>>>", 0, "")),
    ("T hooks mm+hint", mk(True, "mattermost", MM, "<<<prompt>>>", 104, "hooks")),
    ("T hooks mm+nohint", mk(True, "mattermost", None, "<<<prompt>>>", 104, "hooks")),
]:
    p, e = tcall(args)
    direct[tag] = show(tag, p, e)

# Reproduce the API's effective inputs for the no-channel/default case: the
# core injects `_meta` (platform from the channel, here the empty string for a
# default profile with no channel), and the prompt tool prefers `_meta` over
# the argument for `platform`.
p, e = tcall(mk(True, "", None, "<<<prompt>>>", 0, ""),
             {"platform": "", "profile_name": "omni", "channel_id": ""})
direct["T default meta=''"] = show("T default meta=''", p, e)

# ---------- the dashboard-facing API ----------
api = {}
for tag, path in [("API /prompt/default", "/prompt/default"),
                  ("API /prompt/hooks", "/prompt/hooks")]:
    st, raw = call("GET", path)
    if st != 200:
        print("%-24s ERR HTTP%s %s" % (tag, st, raw[:200]))
        continue
    j = json.loads(raw)
    api[tag] = j
    s = j.get("system", "")
    print("%-24s sys_len=%-5d md5=%s keys=%s msgs=%d plan=%s usr=%r"
          % (tag, len(s), md5(s), sorted(j.keys()), len(j.get("messages") or []),
             j.get("plan"), (j.get("user") or "")[:24]))

for a, aj in api.items():
    asys = aj.get("system", "")
    hits = [t for t, dp in direct.items() if dp and dp.get("system") == asys]
    print("  EQ %-20s system matches: %s" % (a, hits or "NONE"))
    for t in hits:
        dp = direct[t]
        print("     MATCHED-BY %s" % t)
        for k in ("memory", "context", "user", "template", "plan"):
            if k in dp or k in aj:
                same = dp.get(k) == aj.get(k)
                if not same:
                    print("     DIFF[%s] %s tool=%r api=%r" % (t, k, dp.get(k), aj.get(k)))
        msgs = aj.get("messages") or []
        if msgs:
            roles = [m.get("role") for m in msgs]
            print("     msgs=%d roles=%s last=%r" % (len(msgs), roles, (msgs[-1].get("content") or "")[:40]))

# ---------- preview POST (plan=true) : must NOT write threads.plan ----------
st, raw = call("POST", "/prompt-preview/hooks", {"prompt": "PARITY-PREVIEW", "plan": True})
try:
    j = json.loads(raw)
    print("POST /prompt-preview/hooks status=%s plan=%s keys=%s sys_len=%d msgs=%d"
          % (st, j.get("plan"), sorted(j.keys()), len(j.get("system") or ""), len(j.get("messages") or [])))
    last = (j.get("messages") or [{}])[-1].get("content", "")
    print("  preview user msg=%r" % last[:60])
except Exception:  # noqa: BLE001
    print("POST preview ERR", st, raw[:200])
print("PARITY_SCRIPT_DONE")
