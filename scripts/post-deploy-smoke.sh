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

echo "== [1/3] gateway health =="
HEALTH="$(curl -s -m 15 -o /dev/null -w '%{http_code}' "$BASE/health" 2>/dev/null || true)"
[ "$HEALTH" = "200" ] || { echo "FAIL: /health returned $HEALTH (expect 200)"; exit 1; }
echo "OK: /health 200"

echo "== [2/3] web SPA asset consistency probe =="
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOCAL_INDEX="$REPO_ROOT/web/dist/index.html"
if [ -f "$LOCAL_INDEX" ]; then
  EXPECTED_ASSET="$(grep -oE 'src="/assets/index-[^"]+\.js"' "$LOCAL_INDEX" | head -1 || true)"
  if [ -n "$EXPECTED_ASSET" ]; then
    ONLINE_HTML="$(curl -s -m 15 "$BASE/" 2>/dev/null || true)"
    if ! echo "$ONLINE_HTML" | grep -Fq "$EXPECTED_ASSET"; then
      echo "::error::[SPA-CACHE-MISMATCH] Production index.html asset does not match expected local build: $EXPECTED_ASSET"
      echo "Production response asset snippet:"
      echo "$ONLINE_HTML" | grep -oE 'src="/assets/index-[^"]+\.js"' || true
      exit 1
    fi
    echo "OK: web SPA asset matches expected build ($EXPECTED_ASSET)"
  else
    echo "SKIP: expected asset pattern not found in local index.html"
  fi
else
  echo "SKIP: local web/dist/index.html not present (running outside full build context)"
fi

echo "== [3/3] minimal completion through $MODEL =="
DELAYS=(2 5 10)
MAX_ATTEMPTS=3
BODY_TMP="$(mktemp)"
trap 'rm -f "$BODY_TMP"' EXIT

ATTEMPT=1
while [ "$ATTEMPT" -le "$MAX_ATTEMPTS" ]; do
  CURL_RC=0
  HTTP_CODE="$(curl -s -m 90 -H "Authorization: Bearer $KEY" -H "Content-Type: application/json" \
    "$BASE/v1/chat/completions" \
    -d "{\"model\":\"$MODEL\",\"messages\":[{\"role\":\"user\",\"content\":\"hi\"}],\"max_tokens\":10,\"stream\":false}" \
    -o "$BODY_TMP" -w '%{http_code}' 2>/dev/null)" || CURL_RC=$?

  RAW_SNIPPET="$(head -c 500 "$BODY_TMP" 2>/dev/null || true)"
  echo "[attempt $ATTEMPT/$MAX_ATTEMPTS] http_code=$HTTP_CODE curl_rc=$CURL_RC body_snippet=$RAW_SNIPPET"

  # 成功路径：200 且 finish_reason ∈ {stop,length}
  if [ "$CURL_RC" -eq 0 ] && [ "$HTTP_CODE" = "200" ]; then
    FINISH="$(python3 -c "import json,sys
try:
    d = json.load(open(sys.argv[1]))
    if d.get('error'):
        print('ERROR:' + json.dumps(d.get('error'), ensure_ascii=False))
    else:
        print(d.get('choices', [{}])[0].get('finish_reason', ''))
except Exception as e:
    print('PARSE_ERROR:' + str(e))
" "$BODY_TMP" 2>/dev/null || echo "PARSE_ERROR")"

    case "$FINISH" in
      stop|length)
        echo "OK: completion finish=$FINISH"
        echo "PASS: post-deploy smoke OK"
        exit 0
        ;;
      ERROR:*)
        # 200 下返回 payload error 视作网关内部异常，按 5xx 重试逻辑处理
        IS_RETRYABLE=1
        ;;
      *)
        IS_RETRYABLE=1
        ;;
    esac
  elif [ "$CURL_RC" -ne 0 ] || [ "$HTTP_CODE" = "429" ] || [[ "$HTTP_CODE" =~ ^5[0-9]{2}$ ]]; then
    IS_RETRYABLE=1
  elif [[ "$HTTP_CODE" =~ ^4[0-9]{2}$ ]]; then
    # 其它 4xx（如 400 Bad Request / 401 Unauthorized / 403 Forbidden / 404 Not Found）
    echo "SMOKE-BROKEN: http_code=$HTTP_CODE curl_rc=$CURL_RC snippet=$RAW_SNIPPET" >&2
    exit 1
  else
    IS_RETRYABLE=1
  fi

  if [ "$IS_RETRYABLE" -eq 1 ]; then
    if [ "$ATTEMPT" -lt "$MAX_ATTEMPTS" ]; then
      DELAY_SEC="${DELAYS[$((ATTEMPT - 1))]}"
      echo "RETRYABLE failure detected (http_code=$HTTP_CODE, curl_rc=$CURL_RC). Backing off for ${DELAY_SEC}s..."
      sleep "$DELAY_SEC"
      ATTEMPT=$((ATTEMPT + 1))
    else
      echo "SMOKE-TRANSIENT: http_code=$HTTP_CODE curl_rc=$CURL_RC snippet=$RAW_SNIPPET" >&2
      exit 1
    fi
  fi
done

