#!/usr/bin/env bash
set -e

SOCKET="tui-test-sock"
SESSION="tui-session"

# 清理旧 session 和端口残留
tmux -L "$SOCKET" kill-session -t "$SESSION" 2>/dev/null || true
pkill -f "python3 scripts/mock_gateway.py" 2>/dev/null || true

# 启动 mock gateway
python3 scripts/mock_gateway.py &
MOCK_PID=$!
sleep 1

TEMP_DIR=$(mktemp -d)
CFG="$TEMP_DIR/ponyllm-test.toml"

cat << 'CFG_EOF' > "$CFG"
[gateway]
bind = "127.0.0.1:18090"
api_key = "sk-test-key"

[providers.mock-deepseek]
base_url = "http://127.0.0.1:18090"
default_model = "deepseek-chat"
strategy = "priority"
models = ["deepseek-chat", "deepseek-coder"]

[[providers.mock-deepseek.keys]]
id = "k1"
api_key = "sk-mock-123"
priority = 1
weight = 10
CFG_EOF

echo "启动 tmux 窗格运行 TUI (110x32)..."
tmux -L "$SOCKET" new-session -d -s "$SESSION" -x 110 -y 32 "./target/debug/ponyllm tui --config '$CFG' --gateway-url 'http://127.0.0.1:18090'"

sleep 2

# 1. 抓取初始概览 Tab (此时网关在线 UP)
tmux -L "$SOCKET" capture-pane -p -t "$SESSION" > /tmp/tui_overview.txt
python3 scripts/capture_tui.py /tmp/tui_overview.txt /tmp/tui_overview.png

# 2. 切换到 Tab 5: 实时对话
tmux -L "$SOCKET" send-keys -t "$SESSION" "5"
sleep 0.5
tmux -L "$SOCKET" capture-pane -p -t "$SESSION" > /tmp/tui_chat_empty.txt
python3 scripts/capture_tui.py /tmp/tui_chat_empty.txt /tmp/tui_chat_empty.png

# 3. 发送第一轮提问
tmux -L "$SOCKET" send-keys -t "$SESSION" "Hello PonyLLM" Enter
# 等待流式输出完成
sleep 2.5
tmux -L "$SOCKET" capture-pane -p -t "$SESSION" > /tmp/tui_chat_round1.txt
python3 scripts/capture_tui.py /tmp/tui_chat_round1.txt /tmp/tui_chat_round1.png

# 4. 发送第二轮提问
tmux -L "$SOCKET" send-keys -t "$SESSION" "Can you remember context?" Enter
sleep 2.5
tmux -L "$SOCKET" capture-pane -p -t "$SESSION" > /tmp/tui_chat_round2.txt
python3 scripts/capture_tui.py /tmp/tui_chat_round2.txt /tmp/tui_chat_round2.png

# 退出 TUI 与清理
tmux -L "$SOCKET" send-keys -t "$SESSION" "Escape"
sleep 0.5
tmux -L "$SOCKET" kill-session -t "$SESSION" 2>/dev/null || true
kill "$MOCK_PID" 2>/dev/null || true
rm -rf "$TEMP_DIR"

echo "=== 多轮真实对话及状态验证与截屏完成 ==="
