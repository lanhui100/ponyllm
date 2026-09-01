use std::sync::Arc;
use std::time::Duration;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde_json::Value;
use crate::error::{CoreError, Result};
use crate::pool::{ApiKeyEntry, KeyPool, PoolErrorType};

#[derive(Debug, Clone)]
pub struct UpstreamExecutor {
    pub pool: Arc<KeyPool>,
    pub client: reqwest::Client,
    pub max_retries: usize,
}

impl UpstreamExecutor {
    pub fn new(pool: Arc<KeyPool>, max_retries: usize) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_default();

        Self {
            pool,
            client,
            max_retries,
        }
    }

    fn build_headers(&self, key: &ApiKeyEntry) -> Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));

        let clean_key = key.api_key.trim();
        if clean_key.is_empty() {
            return Err(CoreError::Internal(format!("API key for '{}' is empty", key.id)));
        }

        // Bearer header for OpenAI/DeepSeek
        let bearer_val = HeaderValue::from_str(&format!("Bearer {}", clean_key))
            .map_err(|e| CoreError::Internal(format!("Invalid characters in API key for '{}': {}", key.id, e)))?;
        headers.insert(AUTHORIZATION, bearer_val);

        // x-api-key header for Anthropic
        let x_api_val = HeaderValue::from_str(clean_key)
            .map_err(|e| CoreError::Internal(format!("Invalid characters in API key for '{}': {}", key.id, e)))?;
        headers.insert("x-api-key", x_api_val);

        Ok(headers)
    }

    /// Execute a JSON request with transparent automatic failover before response body starts
    pub async fn execute_json_request(&self, url: &str, body: &Value) -> Result<Value> {
        let mut last_error = String::new();

        for attempt in 0..self.max_retries.max(1) {
            let key = match self.pool.select_key() {
                Ok(k) => k,
                Err(e) => {
                    return Err(CoreError::AllRetriesFailed {
                        retries: attempt,
                        last_error: format!("No available key in pool: {}", e),
                    });
                }
            };

            let headers = match self.build_headers(&key) {
                Ok(h) => h,
                Err(e) => {
                    self.pool.record_error(&key.id, PoolErrorType::AuthInvalid);
                    last_error = e.to_string();
                    continue;
                }
            };

            let req = self.client.post(url).headers(headers).json(body);

            match req.send().await {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        self.pool.record_success(&key.id);
                        let json_val = resp.json::<Value>().await?;
                        return Ok(json_val);
                    }

                    // Handle failover status codes
                    let status_code = status.as_u16();
                    let retry_after = resp
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|s| s.parse::<u64>().ok())
                        .map(Duration::from_secs);

                    let err_body = resp.text().await.unwrap_or_default();
                    last_error = format!("HTTP {} from {}: {}", status_code, key.id, err_body);

                    if status_code == 429 {
                        self.pool.record_error(&key.id, PoolErrorType::RateLimit { retry_after });
                    } else if status_code == 401 {
                        self.pool.record_error(&key.id, PoolErrorType::AuthInvalid);
                    } else if status_code == 402 || (status_code == 403 && err_body.to_lowercase().contains("quota")) {
                        self.pool.record_error(&key.id, PoolErrorType::QuotaExhausted);
                    } else if status.is_server_error() {
                        self.pool.record_error(&key.id, PoolErrorType::ServerError);
                    } else {
                        // Client error that is not retryable (e.g. 400 Bad Request)
                        return Err(CoreError::UpstreamStatusError {
                            status,
                            body: err_body,
                        });
                    }
                }
                Err(err) => {
                    last_error = format!("Network error with {}: {}", key.id, err);
                    self.pool.record_error(&key.id, PoolErrorType::NetworkError);
                }
            }
        }

        Err(CoreError::AllRetriesFailed {
            retries: self.max_retries,
            last_error,
        })
    }

    /// Execute a streaming request with failover before the first SSE chunk is yielded
    pub async fn execute_stream_request(&self, url: &str, body: &Value) -> Result<reqwest::Response> {
        let mut last_error = String::new();

        for attempt in 0..self.max_retries.max(1) {
            let key = match self.pool.select_key() {
                Ok(k) => k,
                Err(e) => {
                    return Err(CoreError::AllRetriesFailed {
                        retries: attempt,
                        last_error: format!("No available key in pool: {}", e),
                    });
                }
            };

            let headers = match self.build_headers(&key) {
                Ok(h) => h,
                Err(e) => {
                    self.pool.record_error(&key.id, PoolErrorType::AuthInvalid);
                    last_error = e.to_string();
                    continue;
                }
            };

            let req = self.client.post(url).headers(headers).json(body);

            match req.send().await {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        self.pool.record_success(&key.id);
                        return Ok(resp);
                    }

                    let status_code = status.as_u16();
                    let retry_after = resp
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|s| s.parse::<u64>().ok())
                        .map(Duration::from_secs);

                    let err_body = resp.text().await.unwrap_or_default();
                    last_error = format!("HTTP {} from {}: {}", status_code, key.id, err_body);

                    if status_code == 429 {
                        self.pool.record_error(&key.id, PoolErrorType::RateLimit { retry_after });
                    } else if status_code == 401 {
                        self.pool.record_error(&key.id, PoolErrorType::AuthInvalid);
                    } else if status_code == 402 || (status_code == 403 && err_body.to_lowercase().contains("quota")) {
                        self.pool.record_error(&key.id, PoolErrorType::QuotaExhausted);
                    } else if status.is_server_error() {
                        self.pool.record_error(&key.id, PoolErrorType::ServerError);
                    } else {
                        return Err(CoreError::UpstreamStatusError {
                            status,
                            body: err_body,
                        });
                    }
                }
                Err(err) => {
                    last_error = format!("Network error with {}: {}", key.id, err);
                    self.pool.record_error(&key.id, PoolErrorType::NetworkError);
                }
            }
        }

        Err(CoreError::AllRetriesFailed {
            retries: self.max_retries,
            last_error,
        })
    }
}
