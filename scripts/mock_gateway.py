import http.server
import socketserver
import json
import time

class MockOpenAIHandler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == '/health':
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(b'{"status":"ok"}')
        elif self.path == '/v1/telemetry/metrics':
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(b'{"total_requests":42,"success_requests":42,"error_requests":0,"status_429":0}')
        elif self.path == '/v1/telemetry/stream':
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(b'{"stream_flow":null}')
        elif self.path == '/v1/telemetry/recorder':
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(b'[]')
        else:
            self.send_response(404)
            self.end_headers()

    def do_POST(self):
        if self.path == '/v1/chat/completions':
            content_length = int(self.headers.get('Content-Length', 0))
            body_bytes = self.rfile.read(content_length)
            req = json.loads(body_bytes.decode('utf-8'))
            messages = req.get('messages', [])
            last_msg = messages[-1]['content'] if messages else 'Hello'

            reply_text = f"[PonyLLM 智能回复]: 收到您的提问 '{last_msg}'。当前多轮会话上下文共 {len(messages)} 条消息。"

            self.send_response(200)
            self.send_header('Content-Type', 'text/event-stream')
            self.send_header('Cache-Control', 'no-cache')
            self.end_headers()

            # 发送流式 SSE
            chunks = [reply_text[i:i+4] for i in range(0, len(reply_text), 4)]
            for chunk in chunks:
                data = {
                    "id": "chatcmpl-mock",
                    "object": "chat.completion.chunk",
                    "choices": [{
                        "index": 0,
                        "delta": {"content": chunk},
                        "finish_reason": None
                    }]
                }
                self.wfile.write(f"data: {json.dumps(data, ensure_ascii=False)}\n\n".encode('utf-8'))
                self.wfile.flush()
                time.sleep(0.03)

            self.wfile.write(b"data: [DONE]\n\n")
            self.wfile.flush()
        else:
            self.send_response(404)
            self.end_headers()

    def log_message(self, format, *args):
        pass

if __name__ == '__main__':
    socketserver.TCPServer.allow_reuse_address = True
    with socketserver.TCPServer(("127.0.0.1", 18090), MockOpenAIHandler) as httpd:
        print("Mock server serving at 18090")
        httpd.serve_forever()
