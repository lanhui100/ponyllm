#!/usr/bin/env bash
# 验收脚本 / 契约漂移门禁（离线，不需要集群）
#
# 背景（B019 事故，2026-10-23）：`scripts/phase3-verify.sh` 长期引用
# `PUT /api/admin/strategy`，而该端点早在 wave-2 从契约移除，导致发布验收在
# [6/7] 直接 404 —— 直到 v0.2.50 发布才被发现（脚本只在人工发布时跑，CI 无人管）。
# 本门禁把这类漂移左移到 PR 阶段：
#   1. 所有 scripts/*.sh § deploy/*.sh § tests/pre-flight/*.sh 通过 `bash -n`；
#   2. scripts/*.sh（非注释行）引用的 `/api/...` 字面路径必须能在 `web/openapi.json` 中找到
#      （前缀匹配，容忍 `$VAR` 拼接与路径参数）；
#   3. 脚本中若出现 `deploy/<name>` 形式的 *单名* 网关 Deployment 引用，必须能在
#      `deploy/ponyllm-deployment.yaml` 中找到同名对象（防 B019 的 `ponyllm-gateway`
#      NotFound 类漂移）。
#
# Usage: bash scripts/check-verify-contract-drift.sh
# Exit: 0 = 通过；1 = 发现漂移（打印 file:path 明细）
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

echo "== [1/3] bash 语法检查 =="
SYNTAX_FAIL=0
for f in scripts/*.sh deploy/*.sh tests/pre-flight/*.sh; do
  [ -f "$f" ] || continue
  if ! bash -n "$f" 2>/tmp/drift-syntax.err; then
    echo "FAIL syntax: $f"
    sed 's/^/    /' /tmp/drift-syntax.err
    SYNTAX_FAIL=1
  fi
done
rm -f /tmp/drift-syntax.err
[ "$SYNTAX_FAIL" = "0" ] || { echo "FAIL: bash -n 失败"; exit 1; }
echo "OK all shell scripts parse"

echo "== [2/3] API 端点漂移（脚本引用 vs web/openapi.json 契约） =="
python3 - <<'PY'
import glob, json, re, sys

spec = json.load(open("web/openapi.json", encoding="utf-8"))
paths = list(spec.get("paths", {}).keys())
# 归一化：去掉尾部斜杠与路径参数差异；用于前缀匹配
norm = lambda p: p.rstrip("/")

ref_re = re.compile(r"/api/[A-Za-z0-9_\-/{}.:]*")
missing = {}
SELF = "scripts/check-verify-contract-drift.sh"
for f in sorted(glob.glob("scripts/*.sh") + glob.glob("tests/pre-flight/*.sh")):
    if f == SELF:      # 本门禁自身含示例路径，跳过自检
        continue
    try:
        raw = open(f, encoding="utf-8", errors="ignore").read()
    except OSError:
        continue
    # 只扫非注释行：去掉 ` #` 之后的内容（保留 shebang 与 URL 中的 #fragment 语义：
    # 仅当前面是空格/Tab 时视为注释起点）
    txt = "\n".join(re.sub(r"(^|[ \t])#.*$", r"\1", ln) for ln in raw.splitlines())
    for m in ref_re.findall(txt):
        p = norm(m)
        # 去掉脚本拼接残留（如 /api/admin/providers/ + "$N" 已被正则截断）与占位符
        if len(p) < len("/api/x") or "..." in p or p.endswith("/x"):
            continue
        ok = any(norm(q) == p or norm(q).startswith(p + "/") or p.startswith(norm(q) + "/")
                 for q in paths)
        if not ok:
            missing.setdefault(f, set()).add(p)

if missing:
    print("FAIL 以下脚本引用了契约中不存在的 API 路径：")
    for f, ps in sorted(missing.items()):
        for p in sorted(ps):
            print(f"    {f}: {p}")
    print("  修复：改用现存端点，或同步 web/openapi.json（跑 admin_contract_tests 的 dump_openapi_json）")
    sys.exit(1)
print(f"OK all referenced API paths exist in contract ({len(paths)} paths)")
PY

echo "== [3/3] Deployment 名称漂移（脚本引用 vs deploy/ponyllm-deployment.yaml） =="
python3 - <<'PY'
import glob, re, sys

manifest = open("deploy/ponyllm-deployment.yaml", encoding="utf-8").read()
declared = set(re.findall(r"^\s*name:\s*(ponyllm-gateway[a-z0-9\-]*)", manifest, re.M))
singles = {}
for f in sorted(glob.glob("scripts/*.sh")):
    txt = open(f, encoding="utf-8", errors="ignore").read()
    for m in re.findall(r"deploy[/ ]+(ponyllm-gateway[a-z0-9\-]*)", txt):
        if m.endswith("-"):   # 前缀拼接（如 ponyllm-gateway-$role），非单名引用
            continue
        if m not in declared:
            singles.setdefault(f, set()).add(m)

if singles:
    print("FAIL 以下脚本引用了清单中不存在的 Deployment：")
    for f, names in sorted(singles.items()):
        for n in sorted(names):
            print(f"    {f}: {n}")
    print(f"  清单中声明: {sorted(declared)}")
    sys.exit(1)
print(f"OK all referenced deployments declared ({sorted(declared)})")
PY

echo "== PASS: 脚本语法 / API 契约 / Deployment 名称均无漂移 =="
