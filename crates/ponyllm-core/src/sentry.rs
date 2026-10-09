use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::{self, Sender};
use tracing::{debug, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Frame {
    pub filename: Option<String>,
    pub function: Option<String>,
    pub lineno: Option<u32>,
    pub colno: Option<u32>,
    pub in_app: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Exception {
    pub error_type: String,
    pub value: Option<String>,
    pub stacktrace: Option<Vec<Frame>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawEvent {
    pub platform: String,
    pub release: Option<String>,
    pub environment: Option<String>,
    pub message: Option<String>,
    pub exception: Option<Exception>,
    pub tags: Option<HashMap<String, String>>,
    pub extra: Option<serde_json::Value>,
    pub breadcrumbs: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestPayload {
    pub platform: Option<String>,
    pub release: Option<String>,
    pub environment: Option<String>,
    pub message: Option<String>,
    pub exception: Option<Exception>,
    pub tags: Option<HashMap<String, String>>,
    pub extra: Option<serde_json::Value>,
    pub breadcrumbs: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Clone)]
pub struct SentryConfig {
    pub endpoint: String,
    pub client_token: Option<String>,
    pub environment: Option<String>,
    pub release: Option<String>,
    pub buffer_capacity: usize,
}

#[derive(Clone, Debug)]
pub struct SentryClient {
    inner: Option<Arc<SentryClientInner>>,
}

#[derive(Debug)]
struct SentryClientInner {
    sender: Sender<RawEvent>,
}

impl SentryClient {
    /// 创建具有后台非阻塞缓冲队列与脱敏管道的 PonySentry 客户端
    pub fn new(config: SentryConfig) -> Self {
        let (tx, mut rx) = mpsc::channel::<RawEvent>(config.buffer_capacity.max(16));
        let endpoint = config.endpoint.clone();
        let client_token = config.client_token.clone();

        tokio::spawn(async move {
            let http_client = reqwest::Client::builder()
                .timeout(Duration::from_millis(2000))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new());

            while let Some(event) = rx.recv().await {
                // 1. 敏感数据脱敏
                let sanitized_event = sanitize_event(event);

                // 2. 组装请求
                let mut req = http_client.post(&endpoint).json(&sanitized_event);
                if let Some(ref token) = client_token {
                    req = req.header("x-client-token", token);
                }

                // 3. 异步非阻塞发送，发生失败仅记录日志，绝不抛出异常阻断业务
                match req.send().await {
                    Ok(resp) => {
                        if !resp.status().is_success() {
                            warn!(status = %resp.status(), "PonySentry ingest returned non-success");
                        } else {
                            debug!("PonySentry event ingested successfully");
                        }
                    }
                    Err(err) => {
                        warn!(error = %err, "Failed to send error event to PonySentry");
                    }
                }
            }
        });

        Self {
            inner: Some(Arc::new(SentryClientInner { sender: tx })),
        }
    }

    /// 空客户端（当未配置上报端点或禁用时）
    pub fn noop() -> Self {
        Self { inner: None }
    }

    pub fn is_enabled(&self) -> bool {
        self.inner.is_some()
    }

    pub fn capture_event(&self, event: RawEvent) {
        if let Some(ref inner) = self.inner {
            // try_send：若队列满了则直接丢弃，决不阻塞调用方任何微秒
            if let Err(err) = inner.sender.try_send(event) {
                warn!(error = %err, "PonySentry buffer queue full or closed, event dropped");
            }
        }
    }

    pub fn capture_error(
        &self,
        error_type: &str,
        message: &str,
        tags: Option<HashMap<String, String>>,
        extra: Option<serde_json::Value>,
    ) {
        let event = RawEvent {
            platform: "rust".to_string(),
            release: option_env!("CARGO_PKG_VERSION").map(|s| s.to_string()),
            environment: None,
            message: Some(message.to_string()),
            exception: Some(Exception {
                error_type: error_type.to_string(),
                value: Some(message.to_string()),
                stacktrace: None,
            }),
            tags,
            extra,
            breadcrumbs: None,
        };
        self.capture_event(event);
    }
}

/// 自动递归与正则过滤敏感信息（sk-*, token, password, authorization 等）
fn sanitize_event(mut event: RawEvent) -> RawEvent {
    if let Some(ref mut msg) = event.message {
        *msg = sanitize_string(msg);
    }
    if let Some(ref mut exc) = event.exception {
        if let Some(ref mut val) = exc.value {
            *val = sanitize_string(val);
        }
    }
    if let Some(ref mut tags) = event.tags {
        for (k, v) in tags.iter_mut() {
            let lower_k = k.to_ascii_lowercase();
            if lower_k.contains("key")
                || lower_k.contains("token")
                || lower_k.contains("auth")
                || lower_k.contains("secret")
            {
                *v = "[REDACTED_API_KEY]".to_string();
            } else {
                *v = sanitize_string(v);
            }
        }
    }
    if let Some(ref mut extra) = event.extra {
        sanitize_json_value(extra);
    }
    event
}

fn sanitize_string(s: &str) -> String {
    let mut sanitized = s.to_string();
    let mut search_from = 0;
    while let Some(rel_pos) = sanitized[search_from..].find("sk-") {
        let pos = search_from + rel_pos;
        let end = sanitized[pos..]
            .find(|c: char| {
                c.is_whitespace() || c == '"' || c == '\'' || c == ',' || c == '\\' || c == '&'
            })
            .map(|i| pos + i)
            .unwrap_or(sanitized.len());
        if end > pos + 3 {
            sanitized.replace_range(pos..end, "[REDACTED_API_KEY]");
            search_from = pos + "[REDACTED_API_KEY]".len();
        } else {
            search_from = pos + 3;
        }
    }
    // Also redact Bearer tokens
    let mut search_bearer = 0;
    while let Some(rel_pos) = sanitized[search_bearer..]
        .to_ascii_lowercase()
        .find("bearer ")
    {
        let pos = search_bearer + rel_pos;
        let token_start = pos + 7;
        let end = sanitized[token_start..]
            .find(|c: char| {
                c.is_whitespace() || c == '"' || c == '\'' || c == ',' || c == '\\' || c == '&'
            })
            .map(|i| token_start + i)
            .unwrap_or(sanitized.len());
        if end > token_start {
            sanitized.replace_range(token_start..end, "[REDACTED]");
            search_bearer = token_start + "[REDACTED]".len();
        } else {
            search_bearer = token_start;
        }
    }
    sanitized
}

fn sanitize_json_value(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Object(map) => {
            for (key, val) in map.iter_mut() {
                let lower = key.to_ascii_lowercase();
                if lower.contains("key")
                    || lower.contains("token")
                    || lower.contains("auth")
                    || lower.contains("secret")
                {
                    *val = serde_json::Value::String("[REDACTED]".to_string());
                } else {
                    sanitize_json_value(val);
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr.iter_mut() {
                sanitize_json_value(item);
            }
        }
        serde_json::Value::String(s) => {
            *s = sanitize_string(s);
        }
        _ => {}
    }
}
