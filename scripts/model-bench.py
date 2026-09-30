#!/usr/bin/env python3
"""Model benchmark: TTFB / TTFT / token throughput via ponyllm gateway (SSE stream).

Usage:
  export PONYLLM_API_KEY=<gateway key>
  python3 scripts/model-bench.py <model> [runs] [max_tokens] [prompt_tail]

Metrics per run (JSON line):
  http_code, ttfb_ms (headers), ttf_body_ms (first body byte), ttft_ms (first content
  delta), total_ms, tokens, tok_per_sec (over total), stream_tps (excl. TTFT),
  max_gap_ms (largest inter-chunk gap), stall_count (gap>2000ms), error
"""
import http.client, json, os, sys, time, re

BASE = os.environ.get("PONYLLM_BASE_URL", "https://tokens.ponyjob.top")
KEY = os.environ.get("PONYLLM_API_KEY", "").strip()
MODEL = sys.argv[1] if len(sys.argv) > 1 else "deepseek-v4-flash"
RUNS = int(sys.argv[2]) if len(sys.argv) > 2 else 5
MAX_TOKENS = int(sys.argv[3]) if len(sys.argv) > 3 else 200
TAIL = sys.argv[4] if len(sys.argv) > 4 else ""
PROMPT = ("用中文写一段 250 字左右的短文，主题是人工智能与创造力，要求有具体例子。"
          + TAIL).strip()

def bench_once(seq):
    body = json.dumps({
        "model": MODEL,
        "messages": [{"role": "user", "content": PROMPT}],
        "max_tokens": MAX_TOKENS, "stream": True, "temperature": 0.7,
    }).encode()
    host = BASE.split("://")[1]
    conn = http.client.HTTPSConnection(host, timeout=180)
    t0 = time.monotonic()
    t_headers = None; t_first_byte = None; t_first_token = None; t_end = None
    tokens = 0; chunks = 0; max_gap = 0.0; gaps_over_2s = 0
    last = None; err = None; http_code = None; finish = None
    try:
        conn.request("POST", "/v1/chat/completions", body=body, headers={
            "Authorization": f"Bearer {KEY}",
            "Content-Type": "application/json",
            "Accept": "text/event-stream",
        })
        resp = conn.getresponse()
        http_code = resp.status
        t_headers = time.monotonic()
        while True:
            line = resp.readline()
            if not line:
                break
            now = time.monotonic()
            if t_first_byte is None:
                t_first_byte = now
            if last is not None:
                gap = now - last
                if gap > max_gap:
                    max_gap = gap
                if gap > 2.0:
                    gaps_over_2s += 1
            last = now
            text = line.decode("utf-8", "ignore").strip()
            if not text:
                continue
            if text.startswith("data:"):
                payload = text[5:].strip()
                if payload == "[DONE]":
                    break
                try:
                    obj = json.loads(payload)
                except Exception:
                    continue
                if "error" in obj:
                    err = json.dumps(obj["error"], ensure_ascii=False)[:200]
                    break
                if obj.get("choices"):
                    ch = obj["choices"][0]
                    delta = ch.get("delta") or {}
                    if delta.get("content"):
                        if t_first_token is None:
                            t_first_token = now
                        tokens += 1
                    if ch.get("finish_reason"):
                        finish = ch["finish_reason"]
                if obj.get("usage") and obj["usage"].get("completion_tokens"):
                    tokens = obj["usage"]["completion_tokens"]
                chunks += 1
        t_end = time.monotonic()
    except Exception as e:
        err = f"{type(e).__name__}: {e}"[:300]
    finally:
        try:
            conn.close()
        except Exception:
            pass
    ttfb = (t_headers - t0) * 1000 if t_headers else None
    tfb = (t_first_byte - t0) * 1000 if t_first_byte else None
    ttft = (t_first_token - t0) * 1000 if t_first_token else None
    total = (t_end - t0) * 1000 if t_end else None
    tps = (tokens / (total / 1000)) if total and total > 0 else None
    stps = (tokens / (t_end - t_first_token)) if (t_end and t_first_token and (t_end - t_first_token) > 0) else None
    return {
        "run": seq, "model": MODEL, "http_code": http_code,
        "ttfb_ms": round(ttfb, 1) if ttfb else None,
        "ttf_body_ms": round(tfb, 1) if tfb else None,
        "ttft_ms": round(ttft, 1) if ttft else None,
        "total_ms": round(total, 1) if total else None,
        "tokens": tokens, "finish": finish,
        "tok_per_sec": round(tps, 2) if tps else None,
        "stream_tps": round(stps, 2) if stps else None,
        "max_gap_ms": round(max_gap * 1000, 1) if max_gap else 0.0,
        "stall_count": gaps_over_2s,
        "error": err,
    }

if __name__ == "__main__":
    if not KEY:
        print("FATAL: PONYLLM_API_KEY unset", file=sys.stderr)
        sys.exit(2)
    rows = []
    for i in range(1, RUNS + 1):
        r = bench_once(i)
        rows.append(r)
        print(json.dumps(r, ensure_ascii=False))
        time.sleep(1.5)
    # summary
    ok = [r for r in rows if r["error"] is None and r["http_code"] == 200 and r["ttft_ms"] is not None]
    def med(vals):
        vals = sorted(v for v in vals if v is not None)
        return vals[len(vals) // 2] if vals else None
    if ok:
        s = {
            "model": MODEL, "runs": RUNS, "ok_runs": len(ok),
            "median_ttfb_ms": med([r["ttfb_ms"] for r in ok]),
            "median_ttft_ms": med([r["ttft_ms"] for r in ok]),
            "median_tps": med([r["tok_per_sec"] for r in ok]),
            "median_stream_tps": med([r["stream_tps"] for r in ok]),
            "median_max_gap_ms": med([r["max_gap_ms"] for r in ok]),
            "min_ttft_ms": min(r["ttft_ms"] for r in ok),
            "max_ttft_ms": max(r["ttft_ms"] for r in ok),
        }
        print("SUMMARY " + json.dumps(s, ensure_ascii=False))
    else:
        print("SUMMARY " + json.dumps({"model": MODEL, "ok_runs": 0, "all_failed": True, "errors": [r["error"] for r in rows]}))
