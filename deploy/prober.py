import http.server
import json
import os
import socketserver
import threading
import time
import urllib.request

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
    }
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

def background_loop():
    while True:
        try:
            run_probe()
        except Exception:
            pass
        time.sleep(PROBE_INTERVAL)

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

if __name__ == "__main__":
    t = threading.Thread(target=background_loop, daemon=True)
    t.start()
    with socketserver.TCPServer((BIND_ADDR, PORT), MetricHandler) as httpd:
        httpd.serve_forever()
