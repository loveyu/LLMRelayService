#!/usr/bin/env python3
"""
LRS Proxy Comparison Test
Tests each model through both the TS proxy (port 8300) and Rust proxy (port 3301),
then compares responses. Requires both proxies running.
"""

import json, http.client, sys, time
from typing import Optional

TS_PORT = 8300
RUST_PORT = 3301
API_KEY = "dev-local-relay-key"
TIMEOUT = 30

MODELS = [
    "glm-5.1",
    "glm-5.2",
    "glm-5",
    "deepseek-v4-pro",
    "deepseek-v4-flash",
    "ai-commit",
    "whiteclouds",
]

def test_model(port: int, model: str, path: str = "/v1/chat/completions") -> tuple[int, Optional[dict], str]:
    """Test a model through a proxy and return (status_code, parsed_json, error)."""
    body = json.dumps({
        "model": model,
        "messages": [{"role": "user", "content": "reply with exactly one word: hello"}],
        "max_tokens": 10,
    })
    headers = {
        "Content-Type": "application/json",
        "x-api-key": API_KEY,
    }
    try:
        conn = http.client.HTTPConnection("127.0.0.1", port, timeout=TIMEOUT)
        conn.request("POST", path, body=body, headers=headers)
        resp = conn.getresponse()
        raw = resp.read().decode("utf-8", errors="replace")
        conn.close()

        # Try parsing as JSON
        try:
            data = json.loads(raw)
            return resp.status, data, ""
        except json.JSONDecodeError:
            # Check if it's an HTML error page
            is_html = raw.strip().startswith("<!") or raw.strip().startswith("<html")
            snippet = raw[:150].replace("\n", " ")
            return resp.status, None, f"non-json({'HTML' if is_html else 'TEXT'}): {snippet}"
    except Exception as e:
        return 0, None, str(e)

def extract_summary(data: Optional[dict]) -> str:
    """Extract a summary from the response."""
    if data is None:
        return "N/A"
    try:
        choices = data.get("choices", [])
        if choices:
            choice = choices[0]
            msg = choice.get("message", {}) or choice.get("delta", {})
            content = msg.get("content", "") or ""
            reasoning = msg.get("reasoning_content", "")
            model = data.get("model", "?")
            finish = choice.get("finish_reason", "?")
            stop = data.get("stop_reason", "")
            usage = data.get("usage", {})
            tokens = f"in={usage.get('input_tokens','?')} out={usage.get('output_tokens','?')}" if usage else ""
            parts = [f"model={model}"]
            if reasoning:
                parts.append(f"reasoning={repr(reasoning[:40])}")
            if content:
                parts.append(f"content={repr(content[:40])}")
            parts.append(f"finish={finish}")
            if tokens:
                parts.append(tokens)
            return "  ".join(parts)
        return f"no choices: {str(data)[:100]}"
    except:
        return f"parse error: {str(data)[:100]}"

# ── Main ──────────────────────────────────────────────────────────────────

print("=" * 80)
print("LRS Proxy Comparison Test")
print(f"  TS  proxy: 127.0.0.1:{TS_PORT}")
print(f"  Rust proxy: 127.0.0.1:{RUST_PORT}")
print("=" * 80)

total = 0
passed = 0
failed = 0

for model in MODELS:
    total += 1
    print(f"\n── {model} ──")

    # Test TS
    ts_status, ts_data, ts_err = test_model(TS_PORT, model)
    print(f"  TS:   HTTP {ts_status:3d}  {extract_summary(ts_data)}")
    if ts_err:
        print(f"        ERROR: {ts_err}")

    # Small delay to avoid rate limiting
    time.sleep(0.2)

    # Test Rust
    rust_status, rust_data, rust_err = test_model(RUST_PORT, model)
    print(f"  Rust: HTTP {rust_status:3d}  {extract_summary(rust_data)}")
    if rust_err:
        print(f"        ERROR: {rust_err}")

    # Compare
    if ts_status == 0 or rust_status == 0:
        print(f"  [SKIP] Connection error")
        continue

    match = True
    issues = []

    if ts_status != rust_status:
        match = False
        issues.append(f"status: TS={ts_status} vs Rust={rust_status}")

    if ts_data and rust_data:
        # Compare model name
        ts_model = ts_data.get("model", "")
        rust_model = rust_data.get("model", "")
        if ts_model != rust_model:
            issues.append(f"model: TS={ts_model} vs Rust={rust_model}")
            # Not a hard fail — model name may legitimately differ

        # Compare has choices
        ts_choices = ts_data.get("choices", [])
        rust_choices = rust_data.get("choices", [])
        if bool(ts_choices) != bool(rust_choices):
            match = False
            issues.append(f"choices: TS has {len(ts_choices)} vs Rust has {len(rust_choices)}")

        # Compare usage
        ts_usage = ts_data.get("usage", {})
        rust_usage = rust_data.get("usage", {})
        if ts_usage and rust_usage:
            ts_tokens = ts_usage.get("total_tokens")
            rust_tokens = rust_usage.get("total_tokens")
            if ts_tokens != rust_tokens and ts_tokens is not None and rust_tokens is not None:
                issues.append(f"tokens: TS={ts_tokens} vs Rust={rust_tokens}")
    elif bool(ts_data) != bool(rust_data):
        match = False
        issues.append(f"JSON: TS={'yes' if ts_data else 'no'} vs Rust={'yes' if rust_data else 'no'}")

    if match:
        print(f"  [PASS] ✓")
        passed += 1
    else:
        print(f"  [FAIL] ✗ {'; '.join(issues)}")
        failed += 1

# ── Summary ───────────────────────────────────────────────────────────────

print(f"\n{'=' * 80}")
print(f"Results: {passed} passed, {failed} failed, {total - passed - failed} skipped (of {total})")
print(f"{'=' * 80}")

if failed > 0:
    sys.exit(1)
