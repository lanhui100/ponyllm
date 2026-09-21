use std::collections::VecDeque;
use std::time::Duration;
use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

pub const MAX_SNIPPET_CHARS: usize = 10 * 1024 * 1024;

/// Independent bound for the `error` free-text field. Upstream error bodies
/// (`HTTP <status> from <key>: <body>`) can carry multi-KB HTML pages; they
/// need far less room than request/response snippets kept for UI inspection.
pub const MAX_ERROR_CHARS: usize = 4 * 1024;

/// Minimum byte length of an `sk-…` run before it is treated as a secret.
/// Short `sk-` mentions in prose (e.g. "sk-abc") are left untouched to avoid
/// over-scrubbing legitimate content.
pub const MIN_SECRET_TOKEN_BYTES: usize = 12;

fn is_secret_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'+' | b'/' | b'=')
}

/// Scrub secrets from free text.
///
/// Upstream error bodies occasionally echo the rejected credential, and user
/// prompts may contain pasted secrets. `sanitize_key` only covers the dedicated
/// key column, so every free-text field (`error`, `request_snippet`,
/// `response_snippet`) passes through here before it is stored in a frame or
/// emitted to the log.
///
/// Covered shapes (P0-8):
/// - `sk-…` runs (existing behavior, `sk-***<last4>`)
/// - `ya29.…` Google OAuth access tokens → `ya29.***`
/// - `1//…` Google OAuth refresh tokens → `1//***`
/// - `Bearer <token>` scheme values → `Bearer ***`
/// - `"refresh_token" / "access_token" / "client_secret"` JSON string values
///   (OAuth request/response echoes) → `"key":"***"`
/// Operates on ASCII boundaries only; non-ASCII bytes terminate a run.
pub fn scrub_secrets(text: &str) -> String {
    let scrubbed = scrub_prefixed_runs(text);
    let scrubbed = scrub_bearer_values(&scrubbed);
    scrub_oauth_json_values(&scrubbed)
}

/// Redact known secret prefixes: `sk-`, `ya29.`, `1//`.
fn scrub_prefixed_runs(text: &str) -> String {
    const PREFIXES: &[(&[u8], &[u8])] = &[
        (b"sk-", b"sk-"),
        (b"ya29.", b"ya29."),
        (b"1//", b"1//"),
    ];
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        let mut matched: Option<(&[u8], usize)> = None;
        for (prefix, _) in PREFIXES {
            if bytes[i..].starts_with(prefix) {
                matched = Some((*prefix, prefix.len()));
                break;
            }
        }
        if let Some((prefix, pre_len)) = matched {
            let mut j = i + pre_len;
            while j < bytes.len() && is_secret_byte(bytes[j]) {
                j += 1;
            }
            if j - i >= MIN_SECRET_TOKEN_BYTES {
                if prefix == b"sk-" {
                    // `bytes[i..j]` is ASCII-only by construction: safe to slice.
                    let tail = &text[j.saturating_sub(4)..j];
                    out.push_str("sk-***");
                    out.push_str(tail);
                } else {
                    out.push_str(std::str::from_utf8(prefix).unwrap_or("***"));
                    out.push_str("***");
                }
                i = j;
                continue;
            }
        }
        // Copy one UTF-8 scalar. `i` always sits on a char boundary here:
        // we only ever advance by ASCII bytes or whole scalar lengths below.
        let ch = text[i..].chars().next().unwrap_or('\u{FFFD}');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Redact `Bearer <token>` scheme values (case-insensitive scheme).
fn scrub_bearer_values(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        let rest = &bytes[i..];
        let scheme_len = if rest.starts_with(b"Bearer ") || rest.starts_with(b"BEARER ") {
            7
        } else if rest.starts_with(b"bearer ") {
            7
        } else {
            0
        };
        if scheme_len > 0 {
            let mut j = i + scheme_len;
            while j < bytes.len() && is_secret_byte(bytes[j]) {
                j += 1;
            }
            if j - (i + scheme_len) >= MIN_SECRET_TOKEN_BYTES {
                out.push_str(&text[i..i + scheme_len]);
                out.push_str("***");
                i = j;
                continue;
            }
        }
        let ch = text[i..].chars().next().unwrap_or('\u{FFFD}');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Redact OAuth JSON string values: `"refresh_token":"…"`,
/// `"access_token":"…"`, `"client_secret":"…"`.
fn scrub_oauth_json_values(text: &str) -> String {
    const KEYS: &[&str] = &["refresh_token", "access_token", "client_secret"];
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        let mut consumed: Option<usize> = None;
        if bytes[i] == b'"' {
            for key in KEYS {
                let pat = format!("\"{}\"", key);
                if text[i..].starts_with(&pat) {
                    let mut j = i + pat.len();
                    while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') {
                        j += 1;
                    }
                    if j < bytes.len() && bytes[j] == b':' {
                        j += 1;
                        while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') {
                            j += 1;
                        }
                        if j < bytes.len() && bytes[j] == b'"' {
                            j += 1;
                            while j < bytes.len() && bytes[j] != b'"' {
                                // Keep `i`/`j` on char boundaries: secret
                                // bytes are ASCII; any non-ASCII aborts.
                                if !bytes[j].is_ascii() {
                                    break;
                                }
                                j += 1;
                            }
                            if j < bytes.len() && bytes[j] == b'"' {
                                out.push_str(&pat);
                                out.push_str(":\"***\"");
                                consumed = Some(j + 1 - i);
                                break;
                            }
                        }
                    }
                }
            }
        }
        if let Some(n) = consumed {
            i += n;
        } else {
            let ch = text[i..].chars().next().unwrap_or('\u{FFFD}');
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

/// True when `s` itself looks like a raw secret (e.g. a user put the real key
/// in as `key_id`). Recorded frames then store the sanitized form instead.
/// Covers `sk-…`, `ya29.…`, and `1//…` runs.
pub fn looks_like_secret(s: &str) -> bool {
    const MARKERS: &[&[u8]] = &[b"sk-", b"ya29.", b"1//"];
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let mut adv = 1;
        for marker in MARKERS {
            if bytes[i..].starts_with(marker) {
                let mut j = i + marker.len();
                while j < bytes.len() && is_secret_byte(bytes[j]) {
                    j += 1;
                }
                if j - i >= MIN_SECRET_TOKEN_BYTES {
                    return true;
                }
                adv = j.max(i + 1) - i;
                break;
            }
        }
        i += adv;
    }
    false
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StreamFlowDetail {
    pub ttft_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub downstream_ttft_ms: Option<f64>,
    pub ttlb_ms: Option<f64>,
    pub chunks: Option<u64>,
    pub bytes: Option<u64>,
    pub max_gap_ms: Option<f64>,
    pub stall_count: Option<u64>,
    pub tps: Option<f64>,
    pub tpot_p50_ms: Option<f64>,
    pub tpot_p95_ms: Option<f64>,
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub cached_tokens: u64,
}

impl From<&super::metrics::StreamFlowSample> for StreamFlowDetail {
    fn from(s: &super::metrics::StreamFlowSample) -> Self {
        Self {
            ttft_ms: s.ttft_ms,
            downstream_ttft_ms: s.downstream_ttft_ms,
            ttlb_ms: Some(s.ttlb_ms),
            chunks: Some(s.chunks),
            bytes: Some(s.bytes),
            max_gap_ms: s.max_gap_ms,
            stall_count: Some(s.stall_count),
            tps: s.tps,
            tpot_p50_ms: s.tpot_p50_ms,
            tpot_p95_ms: s.tpot_p95_ms,
            prompt_tokens: s.prompt_tokens,
            completion_tokens: s.completion_tokens,
            cached_tokens: s.cached_tokens,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FlightFrame {
    pub request_id: String,
    pub endpoint: String,
    /// Upstream provider name (e.g. "deepseek"); kept for TUI display.
    /// `None` preserves the legacy frames that only carried `key_id`.
    pub provider: Option<String>,
    /// Client-requested model (raw virtual name, e.g. "auto" untouched).
    /// Carried for trace display; attribution-neutral.
    pub model: Option<String>,
    pub key_id: String,
    pub raw_key: Option<String>,
    /// Zero-based attempt index within one client request (key retry / provider fallback).
    pub attempt: Option<u32>,
    pub status_code: Option<u16>,
    pub latency: Duration,
    pub error: Option<String>,
    pub request_snippet: Option<String>,
    pub response_snippet: Option<String>,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub cached_tokens: Option<u64>,
    pub ttft_ms: Option<f64>,
    pub downstream_ttft_ms: Option<f64>,
    pub stream_flow: Option<StreamFlowDetail>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedFrame {
    pub request_id: String,
    pub timestamp: DateTime<Utc>,
    pub endpoint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Client-requested model; `None` for legacy frames / non-model events.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub key_id: String,
    pub sanitized_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u32>,
    pub status_code: Option<u16>,
    pub latency_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_snippet: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_snippet: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttft_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub downstream_ttft_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_flow: Option<StreamFlowDetail>,
}

/// Black-box flight recorder with ring buffer & key sanitization
#[derive(Debug)]
pub struct FlightRecorder {
    capacity: usize,
    buffer: RwLock<VecDeque<RecordedFrame>>,
}

impl FlightRecorder {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            buffer: RwLock::new(VecDeque::with_capacity(capacity)),
        }
    }

    pub fn truncate_snippet(s: Option<String>) -> Option<String> {
        s.map(|text| Self::truncate_text(text, MAX_SNIPPET_CHARS))
    }

    fn truncate_text(text: String, limit: usize) -> String {
        let char_count = text.chars().count();
        if char_count > limit {
            let truncated: String = text.chars().take(limit).collect();
            format!("{}...[TRUNCATED]", truncated)
        } else {
            text
        }
    }

    fn scrub_then_truncate(s: Option<String>, limit: usize) -> Option<String> {
        s.map(|raw| Self::truncate_text(scrub_secrets(&raw), limit))
    }

    pub fn record(&self, frame: FlightFrame) {
        // `key_id` is a free-form identifier; if a deployment put a raw secret
        // in as the id, store only its sanitized form.
        let key_id = if looks_like_secret(&frame.key_id) {
            Self::sanitize_key(&frame.key_id)
        } else {
            frame.key_id
        };
        let sanitized_key = Self::sanitize_key(frame.raw_key.as_deref().unwrap_or(&key_id));
        // Free-text fields are scrubbed for secrets BEFORE storage:
        // upstream error bodies may echo the rejected credential, prompts
        // may contain pasted secrets, and responses may echo either.
        // Scrub-then-truncate keeps the stored text within its bound
        // (MAX_SNIPPET_CHARS for snippets, MAX_ERROR_CHARS for errors)
        // after masking.
        let recorded = RecordedFrame {
            request_id: frame.request_id,
            timestamp: Utc::now(),
            endpoint: frame.endpoint,
            provider: frame.provider,
            model: frame.model,
            key_id,
            sanitized_key,
            attempt: frame.attempt,
            status_code: frame.status_code,
            latency_ms: frame.latency.as_millis() as u64,
            error: Self::scrub_then_truncate(frame.error, MAX_ERROR_CHARS),
            request_snippet: Self::scrub_then_truncate(frame.request_snippet, MAX_SNIPPET_CHARS),
            response_snippet: Self::scrub_then_truncate(frame.response_snippet, MAX_SNIPPET_CHARS),
            prompt_tokens: frame.prompt_tokens,
            completion_tokens: frame.completion_tokens,
            cached_tokens: frame.cached_tokens,
            ttft_ms: frame.ttft_ms,
            downstream_ttft_ms: frame.downstream_ttft_ms,
            stream_flow: frame.stream_flow,
        };

        // Every recorded frame (including embedded-SDK usage) leaves a trace in
        // the log. The log line carries metadata + the (scrubbed) error summary
        // ONLY — request/response snippets stay in the in-memory ring and the
        // telemetry endpoint, never in the log file. No `raw_key`, headers or URLs.
        if let Some(err) = recorded.error.as_deref() {
            tracing::warn!(
                request_id = %recorded.request_id,
                endpoint = %recorded.endpoint,
                provider = recorded.provider.as_deref().unwrap_or("-"),
                key_id = %recorded.key_id,
                status = recorded.status_code.unwrap_or(0),
                latency_ms = recorded.latency_ms,
                "flight recorder error frame: {err}"
            );
        } else {
            tracing::debug!(
                request_id = %recorded.request_id,
                endpoint = %recorded.endpoint,
                status = recorded.status_code.unwrap_or(0),
                latency_ms = recorded.latency_ms,
                "flight recorder success frame"
            );
        }

        let mut buf = self.buffer.write();
        // If an existing frame with the same (request_id, attempt) is already present
        // (e.g. StreamStarted updating to StreamCompleted on the same attempt/request),
        // update it in-place rather than leaving the incomplete initial marker.
        // For distinct attempts or failed upstream attempts, never overwrite - preserve each attempt trace!
        let existing_pos = if recorded.attempt.is_some() {
            buf.iter().position(|f| f.request_id == recorded.request_id && f.attempt == recorded.attempt)
        } else {
            // For top-level requests without attempt index:
            // Only update in-place if the previous frame was also a top-level in-flight marker (like StreamStarted)
            // and did NOT record an upstream attempt failure.
            buf.iter().position(|f| f.request_id == recorded.request_id && f.attempt.is_none())
        };

        if let Some(pos) = existing_pos {
            buf[pos] = recorded;
            return;
        }

        if buf.len() >= self.capacity {
            buf.pop_front();
        }
        buf.push_back(recorded);
    }

    pub fn get_recent_frames(&self) -> Vec<RecordedFrame> {
        let buf = self.buffer.read();
        buf.iter().cloned().collect()
    }

    pub fn get_recent_summaries(&self) -> Vec<RecordedFrame> {
        let buf = self.buffer.read();
        buf.iter()
            .map(|f| {
                let mut summary = f.clone();
                // 列表摘要请求去除庞大的长文本 payload，秒级传输极速渲染
                summary.request_snippet = None;
                summary.response_snippet = None;
                summary
            })
            .collect()
    }

    pub fn get_frame(&self, request_id: &str) -> Option<RecordedFrame> {
        let buf = self.buffer.read();
        buf.iter().find(|f| f.request_id == request_id).cloned()
    }

    pub fn sanitize_key(key: &str) -> String {
        let chars: Vec<char> = key.chars().collect();
        let len = chars.len();
        if len <= 8 {
            return "****".to_string();
        }
        let prefix_len = if key.starts_with("sk-") {
            3.min(len)
        } else {
            3.min(len / 4).max(1)
        };
        let suffix_len = 4.min(len / 4).max(1);

        let prefix: String = chars[..prefix_len].iter().collect();
        let suffix: String = chars[len - suffix_len..].iter().collect();
        format!("{}***{}", prefix, suffix)
    }
}

#[cfg(test)]
mod scrub_tests {
    use super::*;

    #[test]
    fn sk_runs_still_scrubbed_with_last4() {
        let out = scrub_secrets("rejected key sk-live-abcdef123456 end");
        assert!(out.contains("sk-***3456"), "got {}", out);
        assert!(!out.contains("sk-live-abcdef123456"));
    }

    #[test]
    fn google_oauth_material_scrubbed() {
        let access = "ya29.a0AdMD6Einf3FwekkOnCpHNv8u3_j2qDn2ADGX5t";
        let refresh = "1//04mock_oauth_refresh_token_for_testing_00000000000000";
        let out = scrub_secrets(&format!("token {} refresh {}", access, refresh));
        assert!(!out.contains(access), "got {}", out);
        assert!(!out.contains(refresh), "got {}", out);
        assert!(out.contains("ya29.***"), "got {}", out);
        assert!(out.contains("1//***"), "got {}", out);
    }

    #[test]
    fn bearer_and_oauth_json_values_scrubbed() {
        let out = scrub_secrets(r#"Authorization: Bearer ya29.Ci6sh0rtt0k3nabcdef {"refresh_token": "1//0secretstuff", "client_secret": "GOCSPX-topsecret"}"#);
        assert!(!out.contains("ya29.Ci6sh0rtt0k3nabcdef"), "got {}", out);
        assert!(!out.contains("1//0secretstuff"), "got {}", out);
        assert!(!out.contains("GOCSPX-topsecret"), "got {}", out);
        // The ya29 run rule fires before the Bearer rule: still redacted.
        assert!(out.contains("Bearer ya29.***"), "got {}", out);
        assert!(out.contains(r#""refresh_token":"***""#), "got {}", out);
        assert!(out.contains(r#""client_secret":"***""#), "got {}", out);
    }

    #[test]
    fn bearer_branch_covers_non_google_tokens() {
        let out = scrub_secrets("Authorization: Bearer abcdefghijklmnop1234 done");
        assert!(!out.contains("abcdefghijklmnop1234"), "got {}", out);
        assert!(out.contains("Bearer ***"), "got {}", out);
    }

    #[test]
    fn benign_text_untouched() {
        let plain = "hello world, no secrets here (sk-abc is short)";
        assert_eq!(scrub_secrets(plain), plain);
    }

    #[test]
    fn looks_like_secret_covers_google_shapes() {
        assert!(looks_like_secret("sk-live-abcdef123456"));
        assert!(looks_like_secret("ya29.a0AdMD6Einf3FwekkOnCpHNv8u3_j2qDn2ADGX5t"));
        assert!(looks_like_secret("1//04mock_oauth_refresh_token_for_testing_00000000000000"));
        assert!(!looks_like_secret("my-key-id"));
    }

    #[test]
    fn record_scrubs_error_and_response_snippets() {
        // H4 regression: upstream error/response echoes of a credential must
        // not land in the ring in plaintext (same guarantee request_snippet
        // already had).
        let rec = FlightRecorder::new(8);
        let leaked = "sk-live-abcdef123456";
        rec.record(FlightFrame {
            request_id: "h4-test".to_string(),
            endpoint: "/v1/chat/completions".to_string(),
            provider: Some("openai".to_string()),
                model: None,
            key_id: "key-1".to_string(),
            raw_key: None,
            attempt: Some(0),
            status_code: Some(401),
            latency: Duration::from_millis(5),
            error: Some(format!("upstream rejected key {}", leaked)),
            request_snippet: Some(format!("{{\"model\":\"x\",\"key\":\"{}\"}}", leaked)),
            response_snippet: Some(format!("{{\"error\":\"bad key {}\"}}", leaked)),
            prompt_tokens: None,
            completion_tokens: None,
            cached_tokens: None,
            ttft_ms: None,
            downstream_ttft_ms: None,
            stream_flow: None,
        });
        let frame = rec.get_frame("h4-test").expect("frame recorded");
        for field in [
            frame.error.as_deref(),
            frame.request_snippet.as_deref(),
            frame.response_snippet.as_deref(),
        ] {
            let text = field.expect("field present");
            assert!(!text.contains(leaked), "leaked secret persisted: {}", text);
        }
        assert!(frame.error.unwrap().contains("sk-***3456"));
        assert!(frame.response_snippet.unwrap().contains("sk-***3456"));
    }

    #[test]
    fn record_truncates_error_field_to_bound() {
        // Scrub-then-truncate: error text beyond MAX_ERROR_CHARS is cut
        // after masking, so multi-KB upstream HTML bodies cannot amplify
        // ring memory.
        let rec = FlightRecorder::new(8);
        let big = "e".repeat(MAX_ERROR_CHARS + 100);
        rec.record(FlightFrame {
            request_id: "h4-trunc".to_string(),
            endpoint: "/v1/chat/completions".to_string(),
            provider: Some("openai".to_string()),
                model: None,
            key_id: "key-1".to_string(),
            raw_key: None,
            attempt: Some(0),
            status_code: Some(500),
            latency: Duration::from_millis(5),
            error: Some(big),
            request_snippet: None,
            response_snippet: None,
            prompt_tokens: None,
            completion_tokens: None,
            cached_tokens: None,
            ttft_ms: None,
            downstream_ttft_ms: None,
            stream_flow: None,
        });
        let frame = rec.get_frame("h4-trunc").expect("frame recorded");
        let err = frame.error.expect("error present");
        assert!(err.ends_with("...[TRUNCATED]"), "error not truncated: len {}", err.len());
        assert!(err.chars().count() <= MAX_ERROR_CHARS + "...[TRUNCATED]".len());
    }
}
