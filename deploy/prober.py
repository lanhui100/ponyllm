import http.server
import json
import os
import socketserver
import subprocess
import time
import urllib.request

PORT = 9115
TARGET_URL = os.environ.get("PROBE_TARGET", "https://tokens.ponyjob.top/v1/chat/completions")
API_KEY = os.environ["PROBE_API_KEY"]
MODEL = os.environ.get("PROBE_MODEL", "deepseek-flash")

state = {
    "probe_success": 1,
    "probe_duration_seconds": 0.05,
    "probe_http_status": 200,
    "probe_last_timestamp": int(time.time()),
    "total_probes": 0,
    "total_failures": 0
}

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
    state["total_probes"] += 1
    try:
        with urllib.request.urlopen(req, timeout=10) as resp:
            dur = time.time() - start
            body = resp.read().decode("utf-8")
            data = json.loads(body)
            if "choices" in data and len(data["choices"]) > 0:
                state["probe_success"] = 1
                state["probe_duration_seconds"] = dur
                state["probe_http_status"] = resp.status
            else:
                state["probe_success"] = 0
                state["total_failures"] += 1
    except urllib.error.HTTPError as e:
        dur = time.time() - start
        state["probe_duration_seconds"] = dur
        state["probe_http_status"] = e.code
        # 401/400 or other errors
        state["probe_success"] = 0
        state["total_failures"] += 1
    except Exception as e:
        dur = time.time() - start
        state["probe_duration_seconds"] = dur
        state["probe_http_status"] = 0
        state["probe_success"] = 0
        state["total_failures"] += 1
    state["probe_last_timestamp"] = int(time.time())

class MetricHandler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/probe" or self.path == "/probe/run":
            run_probe()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(json.dumps(state).encode("utf-8"))
            return
            
        if self.path == "/metrics":
            output = [
                "# HELP ponyllm_synthetic_probe_success Synthetic inference probe success (1=yes, 0=no)",
                "# TYPE ponyllm_synthetic_probe_success gauge",
                f'ponyllm_synthetic_probe_success{{service="ponyllm",target="{TARGET_URL}"}} {state["probe_success"]}',
                "# HELP ponyllm_synthetic_probe_duration_seconds Synthetic inference latency in seconds",
                "# TYPE ponyllm_synthetic_probe_duration_seconds gauge",
                f'ponyllm_synthetic_probe_duration_seconds{{service="ponyllm",target="{TARGET_URL}"}} {state["probe_duration_seconds"]:.4f}',
                "# HELP ponyllm_synthetic_probe_last_timestamp_seconds Last probe execution epoch",
                "# TYPE ponyllm_synthetic_probe_last_timestamp_seconds gauge",
                f'ponyllm_synthetic_probe_last_timestamp_seconds{{service="ponyllm",target="{TARGET_URL}"}} {state["probe_last_timestamp"]}',
                "# HELP ponyllm_synthetic_probe_total Total number of synthetic probes executed",
                "# TYPE ponyllm_synthetic_probe_total counter",
                f'ponyllm_synthetic_probe_total{{service="ponyllm"}} {state["total_probes"]}',
                "# HELP ponyllm_synthetic_probe_failures_total Total number of synthetic probe failures",
                "# TYPE ponyllm_synthetic_probe_failures_total counter",
                f'ponyllm_synthetic_probe_failures_total{{service="ponyllm"}} {state["total_failures"]}'
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
    run_probe()
    with socketserver.TCPServer(("", PORT), MetricHandler) as httpd:
        httpd.serve_forever()
