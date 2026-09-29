#!/usr/bin/env bash
# GitOps 部署后冒烟门禁：验证生产网关 /health 就绪 + 一次最小推理端到端可用。
#
# 背景（2026-09-29 GitOps 修订）：部署滚动后的人工验证（rollout status + curl）
# 固化为可重复命令；CI 无集群访问权，故供操作者/后续有权限的 runner 调用。
# 只读/只消费最小 token，失败非零退出（供 rollout 失败自动回滚门禁用）。
#
# Usage:
#   export PONYLLM_API_KEY=sk-...          # 网关 admin/inference key
#   export PONYLLM_BASE_URL=https://tokens.ponyjob.top  # 默认值
#   export PONYLLM_MODEL=deepseek-v4-flash # 默认值
#   bash scripts/post-deploy-smoke.sh
set -euo pipefail

BASE="${PONYLLM_BASE_URL:-https://tokens.ponyjob.top}"
MODEL="${PONYLLM_MODEL:-deepseek-v4-flash}"
KEY="${PONYLLM_API_KEY:-}"
command -v curl >/dev/null || { echo "FAIL: curl missing"; exit 1; }
[ -n "$KEY" ] || { echo "FAIL: PONYLLM_API_KEY unset"; exit 1; }

echo "== [1/2] gateway health =="
HEALTH="$(curl -s -m 15 -o /dev/null -w '%{http_code}' "$BASE/health" 2>/dev/null || true)"
[ "$HEALTH" = "200" ] || { echo "FAIL: /health returned $HEALTH (expect 200)"; exit 1; }
echo "OK: /health 200"

echo "== [2/2] minimal completion through $MODEL =="
RESP="$(curl -s -m 90 -H "Authorization: Bearer $KEY" -H "Content-Type: application/json" \
  "$BASE/v1/chat/completions" \
  -d "{\"model\":\"$MODEL\",\"messages\":[{\"role\":\"user\",\"content\":\"hi\"}],\"max_tokens\":10,\"stream\":false}")"
FINISH="$(printf '%s' "$RESP" | python3 -c "import json,sys; d=json.load(sys.stdin); \
print('ERROR:'+json.dumps(d.get('error'),ensure_ascii=False)) if d.get('error') else \
print(d['choices'][0].get('finish_reason',''))" 2>/dev/null)"
case "$FINISH" in
  stop|length) echo "OK: completion finish=$FINISH" ;;
  ERROR:*) echo "FAIL: gateway error $FINISH"; exit 1 ;;
  *) echo "FAIL: unexpected/empty completion"; exit 1 ;;
esac
echo "PASS: post-deploy smoke OK"
