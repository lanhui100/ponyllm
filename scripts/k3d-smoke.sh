#!/usr/bin/env bash
# k3d smoke test for the Kubernetes config backend (multi-node HA, Phase 1).
#
# Verifies against a REAL k3d apiserver (not wiremock):
#   1. a Secret carrying `ponyllm.toml` is served to the store;
#   2. a save with the loaded resourceVersion succeeds;
#   3. a save with a STALE resourceVersion is rejected by the apiserver with
#      409 Conflict, which the store maps to ConfigStoreError::Conflict.
# This is the empirical confirmation of the Phase 2 RBAC verb decision
# (patch + resourceVersion CAS stays valid).
#
# Requires: docker + k3d on PATH. Run locally or as a nightly gate — it is
# intentionally NOT part of CI (crates.io/CI matrix has no docker).
#
# Usage: bash scripts/k3d-smoke.sh
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CLUSTER="ponyllm-k3d-smoke"
NS="ponyllm"
SECRET="ponyllm-live-config"

command -v docker >/dev/null || { echo "FAIL docker missing"; exit 1; }
command -v k3d >/dev/null || { echo "FAIL k3d missing"; exit 1; }
command -v kubectl >/dev/null || { echo "FAIL kubectl missing"; exit 1; }

cleanup() {
  k3d cluster delete "$CLUSTER" >/dev/null 2>&1 || true
}
trap cleanup EXIT

echo "[k3d-smoke] creating cluster $CLUSTER"
k3d cluster create "$CLUSTER" --agents 0 --api-port 127.0.0.1:6555 --wait >/dev/null 2>&1
kubectl create namespace "$NS" >/dev/null 2>&1 || true

# Seed the Secret with the sample config (base64).
SAMPLE_TOML="$(cd "$ROOT_DIR" && cat <<'TOML'
[gateway]
bind = "127.0.0.1:8080"
web_enabled = true
default_strategy = "economy"
TOML
)"
B64="$(printf '%s' "$SAMPLE_TOML" | base64 -w0)"
kubectl -n "$NS" create secret generic "$SECRET" \
  --from-file=<(printf '%s' "$SAMPLE_TOML") --dry-run=client -o yaml > /tmp/ponyllm-smoke-secret.yaml
# --from-file=<(...) is a process substitution: rewrite inline instead.
cat > /tmp/ponyllm-smoke-secret.yaml <<EOF
apiVersion: v1
kind: Secret
metadata:
  name: $SECRET
  namespace: $NS
data:
  ponyllm.toml: $B64
EOF
kubectl -n "$NS" apply -f /tmp/ponyllm-smoke-secret.yaml >/dev/null

echo "[k3d-smoke] running kubernetes_store_k3d_tests against $CLUSTER"
K3D_KUBECONFIG_FILE="/tmp/ponyllm-smoke-kubeconfig.yaml"
k3d kubeconfig get "$CLUSTER" > "$K3D_KUBECONFIG_FILE"
(
  cd "$ROOT_DIR"
  KUBECONFIG="$K3D_KUBECONFIG_FILE" \
    cargo test -p ponyllm-server --test kubernetes_store_k3d_tests -- --ignored --nocapture
)

echo "[k3d-smoke] PASS: patch + resourceVersion CAS confirmed against real apiserver"
