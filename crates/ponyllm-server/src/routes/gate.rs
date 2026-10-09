//! B002: inference-plane double gate (user gate + token gate).
//!
//! Shared by chat/messages/responses. The user gate already exists inline in
//! each handler; this module provides the TOKEN gate (per-key quota +
//! model_limits intersection) plus a model-allowlist match helper reused by
//! both gates.

use crate::state::AppState;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde_json::json;
use std::sync::Arc;

/// Whether `model` matches an allowlist entry (exact name, `*` wildcard, or
/// `prefix/*` wildcard) — shared semantic with `UserQuotaTracker::check_access`.
pub fn model_in_allowlist(allowlist: &[String], model: &str) -> bool {
    allowlist.iter().any(|pattern| {
        if pattern == "*" || pattern == model {
            return true;
        }
        if let Some(prefix) = pattern.strip_suffix('*') {
            if model.starts_with(prefix) {
                return true;
            }
        }
        false
    })
}

/// B002 token gate: for an authenticated gateway key (`x-key-id` injected by
/// `auth_middleware`), enforce
/// 1. token-level model allowlist (`GatewayKeyEntry.model_limits`, intersected
///    with the user allowlist by the handler's user gate) → 403
///    `model_forbidden_for_user`;
/// 2. token-level quota (`GatewayKeyEntry.quota`) → 429
///    `token_quota_exhausted`.
///
/// Returns `Ok(())` when the gate passes (or the caller used a legacy/untracked
/// credential with no self-service key row). Fail-closed: a tracked token with
/// `quota` at/over the limit is rejected even when the user gate passed.
pub fn token_gate(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    model: &str,
) -> Result<(), axum::response::Response> {
    let Some(key_id) = headers
        .get("x-key-id")
        .and_then(|h| h.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    else {
        // Legacy credential (no gateway-key row) — no token gate.
        return Ok(());
    };

    // Snapshot the matching gateway-key entry from the runtime config.
    let entry = {
        let cfg = state.config.read();
        cfg.gateway_keys.iter().find(|k| k.id == key_id).cloned()
    };
    let Some(entry) = entry else {
        // Key row gone (rotated away / deleted) → nothing to gate against;
        // the key family already rejected it upstream if invalid.
        return Ok(());
    };

    // Token-level model allowlist (∩ user allowlist handled by the handler's
    // user gate; here we enforce the token side).
    if let Some(limits) = &entry.model_limits {
        if !limits.is_empty() && !model_in_allowlist(limits, model) {
            return Err((
                StatusCode::FORBIDDEN,
                Json(json!({
                    "error": {
                        "message": format!("model '{model}' is not allowed for this token"),
                        "type": "invalid_request_error",
                        "code": "model_forbidden_for_user"
                    }
                })),
            )
                .into_response());
        }
    }

    // Token-level quota (fail-closed: `used >= quota` rejects).
    match state.token_tracker.check_quota(&key_id, entry.quota) {
        Ok(()) => Ok(()),
        Err(ponyllm_core::TokenQuotaError::QuotaExhausted { key_id, used_tokens, quota_limit }) => {
            Err((
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({
                    "error": {
                        "message": format!(
                            "token '{key_id}' quota exhausted: used {used_tokens} >= limit {quota_limit}"
                        ),
                        "type": "rate_limit_error",
                        "code": "token_quota_exhausted"
                    }
                })),
            )
                .into_response())
        }
        // Unknown token in the tracker: upsert-on-miss keeps the gate honest;
        // treat as pass (the row was removed concurrently — the key family
        // answers 401 for a deleted key anyway).
        Err(ponyllm_core::TokenQuotaError::TokenNotFound { .. }) => Ok(()),
    }
}

/// B002 settlement: record token usage on the SAME key id the gate checked.
pub fn token_record_tokens(state: &Arc<AppState>, headers: &HeaderMap, tokens: u64) {
    if let Some(key_id) = headers
        .get("x-key-id")
        .and_then(|h| h.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        state.token_tracker.record_tokens(&key_id, tokens);
    }
}
