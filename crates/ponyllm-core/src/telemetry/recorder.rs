use std::collections::VecDeque;
use std::time::Duration;
use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

pub const MAX_SNIPPET_CHARS: usize = 512;

#[derive(Debug, Clone)]
pub struct FlightFrame {
    pub request_id: String,
    pub endpoint: String,
    pub key_id: String,
    pub raw_key: Option<String>,
    pub status_code: Option<u16>,
    pub latency: Duration,
    pub error: Option<String>,
    pub request_snippet: Option<String>,
    pub response_snippet: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedFrame {
    pub request_id: String,
    pub timestamp: DateTime<Utc>,
    pub endpoint: String,
    pub key_id: String,
    pub sanitized_key: String,
    pub status_code: Option<u16>,
    pub latency_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_snippet: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_snippet: Option<String>,
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
        let sanitized_key = Self::sanitize_key(frame.raw_key.as_deref().unwrap_or(&frame.key_id));
        let recorded = RecordedFrame {
            request_id: frame.request_id,
            timestamp: Utc::now(),
            endpoint: frame.endpoint,
            key_id: frame.key_id,
            sanitized_key,
            status_code: frame.status_code,
            latency_ms: frame.latency.as_millis() as u64,
            error: frame.error,
            request_snippet: Self::truncate_snippet(frame.request_snippet),
            response_snippet: Self::truncate_snippet(frame.response_snippet),
        };

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
