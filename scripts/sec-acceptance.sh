#!/usr/bin/env bash
# ==============================================================================
# Phase-2 安全修复静态验收（F7–F11）—— Test Agent 于业务实施前编写。
# 每项打印 PASS/FAIL；任一 FAIL 则整体 exit 非零（红相：HEAD 上必须全 FAIL）。
#
# 覆盖：
#   F7  rustls ≥ 0.23.45（workspace Cargo.toml 下限 + Cargo.lock 解析版本）
#   F8  CI audit job（cargo audit + pnpm audit）并入 needs；web/package.json pnpm
#       overrides brace-expansion ≥ 2.1.7
#   F9  install.sh / install.ps1 下载后 sha256 校验；release.yml 生成并上传 .sha256 资产
#   F10 hardening 头：CORP same-origin、COOP same-origin、permissions-policy 补
#       payment、CSP 加 upgrade-insecure-requests（deploy/hardening.yaml 或
#       deploy/ponyllm-ingress-hardening.yaml）
#   F11 app.rs 静态资源 Cache-Control immutable（hashed assets）
# ==============================================================================
set -uo pipefail
cd "$(dirname "$0")/.." || exit 2

FAILS=0
pass() { echo "PASS  $1"; }
fail() { echo "FAIL  $1"; FAILS=$((FAILS + 1)); }

# --- semver 比较（三/两段，x.y 视作 x.y.0）------------------------------------------------
semver_ge() { # $1 >= $2 ?
    local a="$1" b="$2" ai bi
    IFS='.' read -ra A <<< "${a%%-*}"
    IFS='.' read -ra B <<< "${b%%-*}"
    for i in 0 1 2; do
        ai=$((10#${A[$i]:-0}))
        bi=$((10#${B[$i]:-0}))
        if (( ai > bi )); then return 0; fi
        if (( ai < bi )); then return 1; fi
    done
    return 0
}

# 从 Cargo 版本规格中取最低版本（"0.23" / "0.23.45" / ">=0.23.45, <0.24" → 0.23.45）
min_version_of() {
    local spec="$1" tok
    tok="$(echo "$spec" | tr ',' '\n' | head -n1 | tr -d '"' | tr -d ' ')"
    tok="${tok#>=}"
    echo "${tok}"
}

# --- F7 rustls ≥ 0.23.45 ---------------------------------------------------------------
RUSTLS_LINE="$(grep -E '^rustls[[:space:]]*=' Cargo.toml | head -n1 || true)"
if [ -z "$RUSTLS_LINE" ]; then
    fail "F7 workspace Cargo.toml 缺少 rustls 声明"
else
    SPEC="$(echo "$RUSTLS_LINE" | sed -n 's/.*version[[:space:]]*=[[:space:]]*\([^,}]*\).*/\1/p')"
    MIN="$(min_version_of "$SPEC")"
    if semver_ge "$MIN" "0.23.45"; then
        pass "F7 Cargo.toml rustls 下限 ${MIN} ≥ 0.23.45"
    else
        fail "F7 Cargo.toml rustls 下限 ${MIN} < 0.23.45（VULN-04；HEAD=0.23 → FAIL，红相成立）"
    fi
fi

LOCK_RUSTLS="$(awk '/^name = "rustls"$/{f=1} f&&/^version =/{gsub(/"/,"",$3); print $3; exit}' Cargo.lock || true)"
if [ -n "$LOCK_RUSTLS" ] && semver_ge "$LOCK_RUSTLS" "0.23.45"; then
    pass "F7 Cargo.lock 解析 rustls ${LOCK_RUSTLS} ≥ 0.23.45"
else
    fail "F7 Cargo.lock 解析 rustls '${LOCK_RUSTLS:-未解析到}' < 0.23.45（红相成立）"
fi

# --- F8 CI audit job + pnpm overrides ----------------------------------------------------
if [ -f .github/workflows/ci.yml ]; then
    if grep -q 'cargo audit' .github/workflows/ci.yml; then
        pass "F8 ci.yml 含 cargo audit 步骤"
    else
        fail "F8 ci.yml 缺少 cargo audit 步骤（VULN-09；HEAD 无 → FAIL，红相成立）"
    fi
    if grep -q 'pnpm audit' .github/workflows/ci.yml; then
        pass "F8 ci.yml 含 pnpm audit 步骤"
    else
        fail "F8 ci.yml 缺少 pnpm audit 步骤（红相成立）"
    fi
else
    fail "F8 .github/workflows/ci.yml 不存在"
fi

if [ -f web/package.json ]; then
    if grep -q '"overrides"' web/package.json; then
        pass "F8 web/package.json 含 pnpm overrides"
    else
        fail "F8 web/package.json 缺少 pnpm overrides（HEAD 无 → FAIL，红相成立）"
    fi
    if grep -q 'brace-expansion' web/package.json && grep -q '2\.1\.7' web/package.json; then
        pass "F8 web/package.json overrides brace-expansion ≥ 2.1.7"
    else
        fail "F8 web/package.json overrides 未固定 brace-expansion ≥ 2.1.7（红相成立）"
    fi
else
    fail "F8 web/package.json 不存在"
fi

# --- F9 install 校验和 -------------------------------------------------------------------
if [ -f install.sh ]; then
    if grep -qE 'sha256sum|shasum -a 256' install.sh; then
        pass "F9 install.sh 下载后 sha256 校验"
    else
        fail "F9 install.sh 缺少 sha256 校验（VULN-13；HEAD 无 → FAIL，红相成立）"
    fi
else
    fail "F9 install.sh 不存在"
fi

if [ -f install.ps1 ]; then
    if grep -qiE 'Get-FileHash|sha256' install.ps1; then
        pass "F9 install.ps1 下载后 sha256 校验"
    else
        fail "F9 install.ps1 缺少 sha256 校验（红相成立）"
    fi
else
    fail "F9 install.ps1 不存在"
fi

if [ -f .github/workflows/release.yml ]; then
    if grep -qE '\.sha256' .github/workflows/release.yml; then
        pass "F9 release.yml 生成/上传 .sha256 资产"
    else
        fail "F9 release.yml 未生成/上传 .sha256 资产（HEAD 无 → FAIL，红相成立）"
    fi
else
    fail "F9 .github/workflows/release.yml 不存在"
fi

# --- F10 hardening 头 --------------------------------------------------------------------
HARDENING=""
[ -f deploy/hardening.yaml ] && HARDENING="deploy/hardening.yaml"
[ -f deploy/ponyllm-ingress-hardening.yaml ] && HARDENING="deploy/ponyllm-ingress-hardening.yaml"
if [ -n "$HARDENING" ]; then
    if grep -qiE 'Cross-Origin-Resource-Policy[^#]*same-origin' "$HARDENING"; then
        pass "F10 $HARDENING CORP same-origin"
    else
        fail "F10 $HARDENING 缺少 Cross-Origin-Resource-Policy: same-origin（VULN-10；HEAD 无 → FAIL，红相成立）"
    fi
    if grep -qiE 'Cross-Origin-Opener-Policy[^#]*same-origin' "$HARDENING"; then
        pass "F10 $HARDENING COOP same-origin"
    else
        fail "F10 $HARDENING 缺少 Cross-Origin-Opener-Policy: same-origin（红相成立）"
    fi
    if grep -qiE 'Permissions-Policy[^#]*payment' "$HARDENING"; then
        pass "F10 $HARDENING permissions-policy 补 payment"
    else
        fail "F10 $HARDENING permissions-policy 缺少 payment（红相成立）"
    fi
    if grep -qi 'upgrade-insecure-requests' "$HARDENING"; then
        pass "F10 $HARDENING CSP 含 upgrade-insecure-requests"
    else
        fail "F10 $HARDENING CSP 缺少 upgrade-insecure-requests（红相成立）"
    fi
else
    fail "F10 未找到 deploy/hardening.yaml 或 deploy/ponyllm-ingress-hardening.yaml（红相成立）"
fi

# --- F11 静态缓存头 ----------------------------------------------------------------------
if [ -f crates/ponyllm-server/src/app.rs ]; then
    if grep -q 'immutable' crates/ponyllm-server/src/app.rs; then
        pass "F11 app.rs 静态资源 Cache-Control immutable"
    else
        fail "F11 app.rs 缺少 hashed assets 的 Cache-Control immutable（VULN-20；HEAD 无 → FAIL，红相成立）"
    fi
else
    fail "F11 crates/ponyllm-server/src/app.rs 不存在"
fi

echo "----------------------------------------"
if [ "$FAILS" -eq 0 ]; then
    echo "sec-acceptance: 全部通过"
    exit 0
else
    echo "sec-acceptance: ${FAILS} 项失败（红相：HEAD 上失败项即待修复契约）"
    exit 1
fi
