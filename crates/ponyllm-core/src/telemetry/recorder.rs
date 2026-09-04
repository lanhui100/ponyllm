use std::collections::VecDeque;
use std::time::Duration;
use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

pub const MAX_SNIPPET_CHARS: usize = 512;

/// Minimum byte length of an `sk-…` run before it is treated as a secret.
/// Short `sk-` mentions in prose (e.g. "sk-abc") are left untouched to avoid
/// over-scrubbing legitimate content.
pub const MIN_SECRET_TOKEN_BYTES: usize = 12;

fn is_secret_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'+' | b'/' | b'=')
}

/// Scrub `sk-…` style secrets from free text.
///
/// Upstream error bodies occasionally echo the rejected credential, and user
/// prompts may contain pasted secrets. `sanitize_key` only covers the dedicated
/// key column, so every free-text field (`error`, `request_snippet`,
/// `response_snippet`) passes through here before it is stored in a frame or
/// emitted to the log. Runs starting with `sk-` of at least
/// [`MIN_SECRET_TOKEN_BYTES`] bytes become `sk-***<last4>`.
/// Operates on ASCII boundaries only; non-ASCII bytes terminate a run.
pub fn scrub_secrets(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"sk-") {
            let mut j = i + 3;
            while j < bytes.len() && is_secret_byte(bytes[j]) {
                j += 1;
            }
            if j - i >= MIN_SECRET_TOKEN_BYTES {
                // `bytes[i..j]` is ASCII-only by construction: safe to slice.
                let tail = &text[j.saturating_sub(4)..j];
                out.push_str("sk-***");
                out.push_str(tail);
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

/// True when `s` itself looks like a raw secret (e.g. a user put the real key
/// in as `key_id`). Recorded frames then store the sanitized form instead.
pub fn looks_like_secret(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"sk-") {
            let mut j = i + 3;
            while j < bytes.len() && is_secret_byte(bytes[j]) {
                j += 1;
            }
            if j - i >= MIN_SECRET_TOKEN_BYTES {
                return true;
            }
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
    false
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StreamFlowDetail {
    pub ttft_ms: Option<f64>,
    pub ttlb_ms: Option<f64>,
    pub chunks: Option<u64>,
    pub bytes: Option<u64>,
    pub max_gap_ms: Option<f64>,
    pub stall_count: Option<u64>,
    pub tps: Option<f64>,
    pub tpot_p50_ms: Option<f64>,
    pub tpot_p95_ms: Option<f64>,
}

impl From<&super::metrics::StreamFlowSample> for StreamFlowDetail {
    fn from(s: &super::metrics::StreamFlowSample) -> Self {
        Self {
            ttft_ms: s.ttft_ms,
            ttlb_ms: Some(s.ttlb_ms),
            chunks: Some(s.chunks),
            bytes: Some(s.bytes),
            max_gap_ms: s.max_gap_ms,
            stall_count: Some(s.stall_count),
            tps: s.tps,
            tpot_p50_ms: s.tpot_p50_ms,
            tpot_p95_ms: s.tpot_p95_ms,
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
    pub key_id: String,
    pub raw_key: Option<String>,
    /// Zero-based attempt index within one client request (key retry / provider fallback).
    pub attempt: Option<u32>,
    pub status_code: Option<u16>,
    pub latency: Duration,
    pub error: Option<String>,
    pub request_snippet: Option<String>,
    pub response_snippet: Option<String>,
    pub stream_flow: Option<StreamFlowDetail>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedFrame {
    pub request_id: String,
    pub timestamp: DateTime<Utc>,
    pub endpoint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
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
        s.map(|text| {
            let char_count = text.chars().count();
            if char_count > MAX_SNIPPET_CHARS {
                let truncated: String = text.chars().take(MAX_SNIPPET_CHARS).collect();
                format!("{}...[TRUNCATED]", truncated)
            } else {
                text
            }
        })
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
        // Free-text fields are scrubbed for `sk-…` secrets BEFORE storage:
        // upstream error bodies may echo the rejected credential and prompts
        // may contain pasted secrets. Scrub-then-truncate keeps the stored
        // text within MAX_SNIPPET_CHARS after masking.
        let recorded = RecordedFrame {
            request_id: frame.request_id,
            timestamp: Utc::now(),
            endpoint: frame.endpoint,
            provider: frame.provider,
            key_id,
            sanitized_key,
            attempt: frame.attempt,
            status_code: frame.status_code,
            latency_ms: frame.latency.as_millis() as u64,
            error: Self::truncate_snippet(frame.error.map(|e| scrub_secrets(&e))),
            request_snippet: Self::truncate_snippet(frame.request_snippet.map(|s| scrub_secrets(&s))),
            response_snippet: Self::truncate_snippet(frame.response_snippet.map(|s| scrub_secrets(&s))),
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
        if buf.len() >= self.capacity {
            buf.pop_front();
        }
        buf.push_back(recorded);
    }

    pub fn get_recent_frames(&self) -> Vec<RecordedFrame> {
        let buf = self.buffer.read();
        buf.iter().cloned().collect()
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
