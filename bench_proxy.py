#!/usr/bin/env python3
"""LRS Proxy Latency Benchmark"""
import json, http.client, time, statistics, ssl

TS = 8300; RUST = 3301; KEY = "dev-local-relay-key"
ROUNDS = 3; TO = 45

TESTS = [
    ("glm-5.1",           "/v1/chat/completions",
     "glm-5.1",           "opencode.ai",  "/zen/go/v1/chat/completions", True,
     "authorization", "Bearer sk-Z0suf0bsCvWrgVTLJ9Kbq2Ql6SXKPusg8sNBavfeltzUYPq1eEP8r4jR0faZ4SIi"),
    ("deepseek-v4-flash", "/v1/chat/completions",
     "deepseek-v4-flash", "api.deepseek.com", "/chat/completions", True,
     "authorization", "Bearer sk-bbcd4618c6ea4ead9e17d59c0339b547"),
    ("ai-commit",         "/v1/chat/completions",
     "deepseek-v4-flash", "api.deepseek.com", "/chat/completions", True,
     "authorization", "Bearer sk-bbcd4618c6ea4ead9e17d59c0339b547"),
]

BODY = {"messages":[{"role":"user","content":"Say hello."}],"max_tokens":10,"temperature":0}

def http_request(host, port, is_ssl, path, headers, body_bytes):
    """HTTP request with timing. Returns (ttfb_ms, total_ms)."""
    start = time.perf_counter()
    try:
        ctx = None
        if is_ssl:
            ctx = ssl.create_default_context()
            ctx.check_hostname = False; ctx.verify_mode = ssl.CERT_NONE
        conn = http.client.HTTPSConnection(host, port, timeout=TO, context=ctx) if is_ssl \
          else http.client.HTTPConnection(host, port, timeout=TO)
        conn.request("POST", path, body=body_bytes, headers=headers)
        resp = conn.getresponse()
        ttfb = (time.perf_counter() - start) * 1000
        resp.read()
        total = (time.perf_counter() - start) * 1000
        conn.close()
        return ttfb, total
    except Exception as e:
        t = (time.perf_counter() - start) * 1000
        return t, t

def test_direct(host, path, is_ssl, ah, av, model):
    body = {**BODY, "model": model}
    data = json.dumps(body).encode()
    headers = {"Content-Type": "application/json", ah: av, "User-Agent": "curl/8.0"}
    port = 443 if is_ssl else 80
    return http_request(host, port, is_ssl, path, headers, data)

def test_proxy(port, model, path):
    body = {**BODY, "model": model}
    data = json.dumps(body).encode()
    start = time.perf_counter()
    try:
        conn = http.client.HTTPConnection("127.0.0.1", port, timeout=TO)
        conn.request("POST", path, data, {"Content-Type": "application/json", "x-api-key": KEY})
        resp = conn.getresponse()
        ttfb = (time.perf_counter() - start) * 1000
        resp.read()
        total = (time.perf_counter() - start) * 1000
        conn.close()
        return ttfb, total
    except:
        t = (time.perf_counter() - start) * 1000
        return t, t

def fmt(vals):
    return f"{statistics.mean(vals):.0f}ms" if vals else "N/A"

print(f"LRS Latency Benchmark  ({ROUNDS} rounds, TTFB / Total)")
print("=" * 95)
print(f"{'Model':<22s} {'Direct':<24s} {'TS Proxy':<24s} {'Rust Proxy':<22s}")
print(f"{'':22s} {'TTFB':>7s} {'Total':>7s}    {'TTFB':>7s} {'Total':>7s}    {'TTFB':>7s} {'Total':>7s}")
print("-" * 95)

results = []
for pmodel, path, dmodel, dhost, dpath, is_ssl, ah, av in TESTS:
    d_ttfb, d_total, ts_ttfb, ts_total, ru_ttfb, ru_total = [],[],[],[],[],[]
    for _ in range(ROUNDS):
        a,b = test_direct(dhost, dpath, is_ssl, ah, av, dmodel)
        d_ttfb.append(a); d_total.append(b)
        time.sleep(0.3)
        a,b = test_proxy(TS, pmodel, path)
        ts_ttfb.append(a); ts_total.append(b)
        time.sleep(0.3)
        a,b = test_proxy(RUST, pmodel, path)
        ru_ttfb.append(a); ru_total.append(b)
        time.sleep(0.3)
    results.append((pmodel, d_ttfb, d_total, ts_ttfb, ts_total, ru_ttfb, ru_total))
    print(f"{pmodel:<22s} {fmt(d_ttfb):>7s} {fmt(d_total):>7s}    {fmt(ts_ttfb):>7s} {fmt(ts_total):>7s}    {fmt(ru_ttfb):>7s} {fmt(ru_total):>7s}")

print("=" * 95)
print("Overhead vs Direct (total time):")
print(f"{'Model':<22s} {'TS overhead':<16s} {'Rust overhead':<16s} {'Rust vs TS':<12s}")
print("-" * 66)
for pmodel, d_ttfb, d_total, ts_ttfb, ts_total, ru_ttfb, ru_total in results:
    dm = statistics.mean(d_total) if d_total else 0
    tm = statistics.mean(ts_total); rm = statistics.mean(ru_total)
    print(f"{pmodel:<22s} +{tm-dm:.0f}ms{'':<11s}+{rm-dm:.0f}ms{'':<11s}{rm-tm:+.0f}ms")
