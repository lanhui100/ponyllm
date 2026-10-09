import argparse
import http.server
import json
import os
import socket
import socketserver
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid

PORT = int(os.environ.get("PORT", "9115"))
BIND_ADDR = os.environ.get("PROBE_BIND_ADDR", "0.0.0.0")
TARGET_URL = os.environ.get("PROBE_TARGET", "https://tokens.ponyjob.top/v1/chat/completions")
API_KEY = os.environ.get("PROBE_API_KEY", "")
MGMT_TOKEN = os.environ.get("PROBE_MGMT_TOKEN", "")
MODEL = os.environ.get("PROBE_MODEL", "deepseek-flash")
# 探针间隔默认 30s；免费额度型模型（opencode-zen 的 *-free）请由 Deployment
# 显式调大（PROBE_INTERVAL_SECONDS=600，2026-10-03 事故复盘）：30s 主动探测会
# 持续消耗 Console 共享免费窗口，把上游打 429 并触发网关全 key 冷却。
PROBE_INTERVAL = int(os.environ.get("PROBE_INTERVAL_SECONDS", "30"))
# 上游首 token 可达 10~40s（opencode-zen/pproxy 路径实测），10s 硬超时必误报；
# 默认对齐网关 90s TTFB 预算，可用 PROBE_TIMEOUT 覆盖。
PROBE_TIMEOUT = float(os.environ.get("PROBE_TIMEOUT", "90"))

# 契约 C3：长流连续性探针配置
# drain 预算冻结值：85s = preStop 25s + 主进程 drain 60s（terminationGracePeriodSeconds=180）
LONG_STREAM_DRAIN_BUDGET_SECONDS = 85.0
PROBE_LONG_STREAM = os.environ.get("PROBE_LONG_STREAM", "0").lower() in ("1", "true", "yes")
PROBE_LONG_STREAM_INTERVAL = int(os.environ.get("PROBE_LONG_STREAM_INTERVAL_SECONDS", "600"))
PROBE_LONG_STREAM_SECONDS = float(os.environ.get("PROBE_LONG_STREAM_SECONDS", "120"))
PROBE_LONG_STREAM_MODEL = os.environ.get("PROBE_LONG_STREAM_MODEL", MODEL)
PROBE_LONG_STREAM_PROMPT = os.environ.get(
    "PROBE_LONG_STREAM_PROMPT",
    "Count from 1 to 500 slowly, outputting each number on a new line."
)
# NFR: 块间空闲判定，连续 data: 块间隔 > 3000ms (3.0s) 视为 stall
PROBE_INTER_CHUNK_TIMEOUT = float(os.environ.get("PROBE_INTER_CHUNK_TIMEOUT", "3.0"))

state = {
    "probe_success": 1,
    "probe_duration_seconds": 0.05,
    "probe_http_status": 200,
    "probe_status_class": "2xx",
    "probe_last_timestamp": int(time.time()),
    "total_probes": 0,
    "total_failures": 0,
    "failures_by_class": {
        "4xx": 0,
        "5xx": 0,
        "other": 0
    },
    # 契约 C3：长流连续性台账
    "long_stream_total": 0,
    "long_stream_complete": 0,
    "long_stream_cut": 0,
    "long_stream_defects": 0,
    "long_stream_last_result": None
}
lock = threading.Lock()

def get_status_class(status_code: int) -> str:
    if 200 <= status_code < 300:
        return "2xx"
    elif 400 <= status_code < 500:
        return "4xx"
    elif 500 <= status_code < 600:
        return "5xx"
    return "other"

def run_probe():
    payload = json.dumps({
        "model": MODEL,
        "messages": [{"role": "user", "content": "ping"}],
        "max_tokens": 1
    }).encode("utf-8")
    
    req = urllib.request.Request(
        TARGET_URL,
        data=payload,
        headers={
            "Authorization": f"Bearer {API_KEY}",
            "Content-Type": "application/json"
        }
    )
    
    start = time.time()
    success = 0
    status_code = 0
    dur = 0.0
    
    try:
        with urllib.request.urlopen(req, timeout=PROBE_TIMEOUT) as resp:
            dur = time.time() - start
            body = resp.read().decode("utf-8")
            data = json.loads(body)
            status_code = resp.status
            if "choices" in data and len(data["choices"]) > 0:
                success = 1
            else:
                success = 0
    except urllib.error.HTTPError as e:
        dur = time.time() - start
        status_code = e.code
        success = 0
    except Exception:
        dur = time.time() - start
        status_code = 0
        success = 0
        
    s_class = get_status_class(status_code)
    now_ts = int(time.time())
    
    with lock:
        state["total_probes"] += 1
        state["probe_success"] = success
        state["probe_duration_seconds"] = dur
        state["probe_http_status"] = status_code
        state["probe_status_class"] = s_class
        state["probe_last_timestamp"] = now_ts
        if success == 0:
            state["total_failures"] += 1
            if s_class in state["failures_by_class"]:
                state["failures_by_class"][s_class] += 1
            else:
                state["failures_by_class"]["other"] += 1

def judge_long_stream(completed: bool, truncated: bool, duration: float, budget: float = LONG_STREAM_DRAIN_BUDGET_SECONDS) -> tuple[str, bool]:
    """
    契约 C3 截断判据（85s 常数）：
    - 正常完成（completed=True，EOF/流结束） -> PASS, is_defect=False
    - 中断（truncated=True）且 duration >= budget (85.0s) -> clean_truncation PASS, is_defect=False
    - 中断（truncated=True）且 duration < budget (85.0s) -> defect FAIL, is_defect=True
    """
    if completed:
        return "PASS", False
    if truncated:
        if duration >= budget:
            # clean_truncation: drain 预算内完整保护，在预算后预期终止
            return "PASS", False
        else:
            # defect: 在途长流在 drain 预算前被提前掐断
            return "FAIL", True
    return "FAIL", True

def run_long_stream_once(
    target_url: str = None,
    api_key: str = None,
    model: str = None,
    budget: float = LONG_STREAM_DRAIN_BUDGET_SECONDS,
    timeout: float = 120.0,
    max_retries: int = 3,
    prompt: str = None
) -> tuple[dict, int]:
    """
    执行单次长流连续性探测（SSE stream=true）。
    返回 (result_dict, exit_code)。
    exit_code 语义：
      0 = PASS (completed 或 clean_truncation >= budget)
      1 = FAIL (truncated 且 duration < budget = defect)
      2 = INFRA_ERROR (首块前网络层重试耗尽无法建连)
    """
    target = target_url or TARGET_URL
    key = api_key if api_key is not None else API_KEY
    m = model or PROBE_LONG_STREAM_MODEL
    p = prompt or PROBE_LONG_STREAM_PROMPT
    b = float(budget)
    
    # 构造 SSE stream=true 长请求 payload: "stream": true
    payload_data = {
        "model": m,
        "messages": [{"role": "user", "content": p}],
        "stream": True,
        "max_tokens": 4096
    }
    payload_bytes = json.dumps(payload_data).encode("utf-8")
    
    headers = {
        "Content-Type": "application/json"
    }
    if key:
        headers["Authorization"] = f"Bearer {key}"
        
    last_err = None
    
    for attempt in range(1, max_retries + 1):
        started_ts = time.time()
        trace_id = f"ls-{int(started_ts)}-{uuid.uuid4().hex[:8]}"
        completed = False
        truncated = False
        chunks_received = 0
        duration = 0.0
        connected = False
        
        req = urllib.request.Request(target, data=payload_bytes, headers=headers)
        
        try:
            with urllib.request.urlopen(req, timeout=timeout) as resp:
                connected = True
                # NFR: 块间空闲超时守卫（连续块间隔 > 3000ms 判为 stall 中断）
                try:
                    if hasattr(resp, "fp") and hasattr(resp.fp, "raw") and hasattr(resp.fp.raw, "_sock") and resp.fp.raw._sock:
                        resp.fp.raw._sock.settimeout(PROBE_INTER_CHUNK_TIMEOUT)
                except Exception:
                    pass
                
                for raw_line in resp:
                    line = raw_line.decode("utf-8", errors="replace").strip()
                    if not line:
                        continue
                    if line.startswith("data:"):
                        payload_str = line[5:].strip()
                        if payload_str == "[DONE]":
                            completed = True
                            break
                        chunks_received += 1
                        # 检查 choice 内的 finish_reason 或 stop_reason
                        try:
                            chunk_json = json.loads(payload_str)
                            choices = chunk_json.get("choices", [])
                            if choices and choices[0].get("finish_reason") in ("stop", "length"):
                                completed = True
                                break
                        except Exception:
                            pass
                            
                duration = time.time() - started_ts
                if not completed:
                    # 未收到 [DONE] 或 finish_reason 即流结束，判定为中断截断
                    truncated = True
        except (urllib.error.HTTPError, urllib.error.URLError, socket.timeout, TimeoutError, ConnectionResetError, Exception) as e:
            duration = time.time() - started_ts
            last_err = str(e)
            if not connected and chunks_received == 0:
                # 首块前连接/网络失败：按契约退避重试 <=3 次
                if attempt < max_retries:
                    time.sleep(5)
                    continue
                # 重试全部耗尽
                judge = "FAIL"
                result = {
                    "trace_id": trace_id,
                    "started": int(started_ts),
                    "completed": False,
                    "chunks": 0,
                    "chunks_received": 0,
                    "truncated": False,
                    "duration": round(duration, 4),
                    "judge": judge,
                    "error": f"infrastructure connection failure: {last_err}"
                }
                with lock:
                    state["long_stream_total"] += 1
                    state["long_stream_defects"] += 1
                    state["long_stream_last_result"] = result
                return result, 2
            else:
                # 流已建立但传输过程中被中断（异常断开或超时）
                truncated = True
                completed = False

        judge, is_defect = judge_long_stream(completed, truncated, duration, b)
        
        result = {
            "trace_id": trace_id,
            "started": int(started_ts),
            "completed": completed,
            "chunks": chunks_received,
            "chunks_received": chunks_received,
            "truncated": truncated,
            "duration": round(duration, 4),
            "judge": judge
        }
        
        with lock:
            state["long_stream_total"] += 1
            if completed:
                state["long_stream_complete"] += 1
            if truncated:
                state["long_stream_cut"] += 1
                if is_defect:
                    state["long_stream_defects"] += 1
            state["long_stream_last_result"] = result
            
        exit_code = 0 if judge == "PASS" else 1
        return result, exit_code

    # 兜底
    judge, is_defect = judge_long_stream(False, True, 0.0, b)
    result = {
        "trace_id": f"ls-{int(time.time())}-fallback",
        "started": int(time.time()),
        "completed": False,
        "chunks": 0,
        "chunks_received": 0,
        "truncated": True,
        "duration": 0.0,
        "judge": "FAIL",
        "error": str(last_err)
    }
    return result, 1

def background_loop():
    while True:
        try:
            run_probe()
        except Exception:
            pass
        time.sleep(PROBE_INTERVAL)

def background_long_stream_loop():
    while True:
        time.sleep(PROBE_LONG_STREAM_INTERVAL)
        try:
            run_long_stream_once(
                target_url=TARGET_URL,
                api_key=API_KEY,
                model=PROBE_LONG_STREAM_MODEL,
                budget=LONG_STREAM_DRAIN_BUDGET_SECONDS,
                timeout=max(PROBE_LONG_STREAM_SECONDS, 120.0)
            )
        except Exception:
            pass

class MetricHandler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path in ("/probe", "/probe/run"):
            auth = self.headers.get("Authorization", "")
            if not MGMT_TOKEN or auth != f"Bearer {MGMT_TOKEN}":
                self.send_response(403)
                self.send_header("Content-Type", "application/json")
                self.end_headers()
                self.wfile.write(b'{"error":"forbidden: valid mgmt token required"}')
                return
            run_probe()
            with lock:
                body = json.dumps(state).encode("utf-8")
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(body)
            return
            
        # 契约 C3 接缝 S2 备选：HTTP 触发单次长流探针
        if self.path == "/probe/long-stream" or self.path.startswith("/probe/long-stream?"):
            auth = self.headers.get("Authorization", "")
            if not MGMT_TOKEN or auth != f"Bearer {MGMT_TOKEN}":
                self.send_response(403)
                self.send_header("Content-Type", "application/json")
                self.end_headers()
                self.wfile.write(b'{"error":"forbidden: valid mgmt token required"}')
                return
            
            budget = LONG_STREAM_DRAIN_BUDGET_SECONDS
            parsed = urllib.parse.urlparse(self.path)
            qs = urllib.parse.parse_qs(parsed.query)
            if "budget" in qs:
                try:
                    budget = float(qs["budget"][0])
                except Exception:
                    pass
            
            result, code = run_long_stream_once(budget=budget)
            http_status = 200 if code == 0 else 500
            body = json.dumps(result).encode("utf-8")
            self.send_response(http_status)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(body)
            return

        if self.path == "/metrics":
            with lock:
                c_success = state["probe_success"]
                c_dur = state["probe_duration_seconds"]
                c_last = state["probe_last_timestamp"]
                c_total = state["total_probes"]
                c_fail = state["total_failures"]
                c_status = state["probe_http_status"]
                c_class = state["probe_status_class"]
                f_4xx = state["failures_by_class"]["4xx"]
                f_5xx = state["failures_by_class"]["5xx"]
                f_other = state["failures_by_class"]["other"]
                # 契约 C3 长流计数器
                ls_total = state["long_stream_total"]
                ls_complete = state["long_stream_complete"]
                ls_cut = state["long_stream_cut"]
                ls_defects = state["long_stream_defects"]

            output = [
                "# HELP ponyllm_synthetic_probe_success Synthetic inference probe success (1=yes, 0=no)",
                "# TYPE ponyllm_synthetic_probe_success gauge",
                f'ponyllm_synthetic_probe_success{{service="ponyllm",target="{TARGET_URL}",status_class="{c_class}"}} {c_success}',
                "# HELP ponyllm_synthetic_probe_status_code Synthetic inference last HTTP status code",
                "# TYPE ponyllm_synthetic_probe_status_code gauge",
                f'ponyllm_synthetic_probe_status_code{{service="ponyllm",target="{TARGET_URL}"}} {c_status}',
                "# HELP ponyllm_synthetic_probe_duration_seconds Synthetic inference latency in seconds",
                "# TYPE ponyllm_synthetic_probe_duration_seconds gauge",
                f'ponyllm_synthetic_probe_duration_seconds{{service="ponyllm",target="{TARGET_URL}"}} {c_dur:.4f}',
                "# HELP ponyllm_synthetic_probe_last_timestamp_seconds Last probe execution epoch",
                "# TYPE ponyllm_synthetic_probe_last_timestamp_seconds gauge",
                f'ponyllm_synthetic_probe_last_timestamp_seconds{{service="ponyllm",target="{TARGET_URL}"}} {c_last}',
                "# HELP ponyllm_synthetic_probe_total Total number of synthetic probes executed",
                "# TYPE ponyllm_synthetic_probe_total counter",
                f'ponyllm_synthetic_probe_total{{service="ponyllm"}} {c_total}',
                "# HELP ponyllm_synthetic_probe_failures_total Total number of synthetic probe failures",
                "# TYPE ponyllm_synthetic_probe_failures_total counter",
                f'ponyllm_synthetic_probe_failures_total{{service="ponyllm"}} {c_fail}',
                f'ponyllm_synthetic_probe_failures_by_class_total{{service="ponyllm",class="4xx"}} {f_4xx}',
                f'ponyllm_synthetic_probe_failures_by_class_total{{service="ponyllm",class="5xx"}} {f_5xx}',
                f'ponyllm_synthetic_probe_failures_by_class_total{{service="ponyllm",class="other"}} {f_other}',
                # 契约 C3 新增 metrics（行名严格对齐契约矩阵）
                "# HELP ponyllm_synthetic_long_stream_total Total number of synthetic long stream probes executed",
                "# TYPE ponyllm_synthetic_long_stream_total counter",
                f'ponyllm_synthetic_long_stream_total{{service="ponyllm"}} {ls_total}',
                "# HELP ponyllm_synthetic_long_stream_complete_total Total number of completed long stream probes",
                "# TYPE ponyllm_synthetic_long_stream_complete_total counter",
                f'ponyllm_synthetic_long_stream_complete_total{{service="ponyllm"}} {ls_complete}',
                "# HELP ponyllm_synthetic_long_stream_cut_total Total number of truncated long stream probes",
                "# TYPE ponyllm_synthetic_long_stream_cut_total counter",
                f'ponyllm_synthetic_long_stream_cut_total{{service="ponyllm"}} {ls_cut}',
                "# HELP ponyllm_synthetic_long_stream_defects_total Total number of long stream probes terminated before drain budget",
                "# TYPE ponyllm_synthetic_long_stream_defects_total counter",
                f'ponyllm_synthetic_long_stream_defects_total{{service="ponyllm"}} {ls_defects}',
            ]
            body = "\n".join(output) + "\n"
            self.send_response(200)
            self.send_header("Content-Type", "text/plain; version=0.0.4; charset=utf-8")
            self.end_headers()
            self.wfile.write(body.encode("utf-8"))
            return
            
        self.send_response(404)
        self.end_headers()

    def log_message(self, format, *args):
        pass

def parse_cli_args():
    parser = argparse.ArgumentParser(description="PonyLLM Synthetic Prober (Short probe & Long stream gate)")
    parser.add_argument("--long-stream-once", action="store_true", help="Run long stream probe once and exit with code")
    parser.add_argument("--long-stream-check", action="store_true", help="Alias for CI gate long stream check")
    parser.add_argument("--url", default=None, help="Target URL (defaults to PROBE_TARGET)")
    parser.add_argument("--api-key", default=None, help="API Key (defaults to PROBE_API_KEY)")
    parser.add_argument("--model", default=None, help="Model name (defaults to PROBE_LONG_STREAM_MODEL)")
    parser.add_argument("--budget", type=float, default=LONG_STREAM_DRAIN_BUDGET_SECONDS, help="Drain budget in seconds (default 85.0)")
    parser.add_argument("--timeout", type=float, default=120.0, help="Total probe timeout in seconds (default 120.0)")
    parser.add_argument("--prompt", default=None, help="Prompt text")
    args, _ = parser.parse_known_args()
    return parser, args

if __name__ == "__main__":
    if "-h" in sys.argv or "--help" in sys.argv:
        parser, _ = parse_cli_args()
        parser.print_help()
        sys.exit(0)
        
    if "--long-stream-once" in sys.argv or "--long-stream-check" in sys.argv:
        _, args = parse_cli_args()
        result, code = run_long_stream_once(
            target_url=args.url,
            api_key=args.api_key,
            model=args.model,
            budget=args.budget,
            timeout=args.timeout,
            prompt=args.prompt
        )
        # 单行 JSON 输出至 stdout（供 CI 脚本解析）
        print(json.dumps(result))
        sys.exit(code)

    t = threading.Thread(target=background_loop, daemon=True)
    t.start()
    
    if PROBE_LONG_STREAM:
        t_ls = threading.Thread(target=background_long_stream_loop, daemon=True)
        t_ls.start()
        
    with socketserver.TCPServer((BIND_ADDR, PORT), MetricHandler) as httpd:
        httpd.serve_forever()
