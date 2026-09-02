use std::collections::HashMap;
use std::sync::Arc;
use ponyllm_core::error::{CoreError, Result};
use ponyllm_core::executor::UpstreamExecutor;
use ponyllm_core::pool::{ApiKeyEntry, KeyPool, RoutingStrategy};
use ponyllm_protocol::anthropic::messages::{MessageRequest, MessageResponse};
use ponyllm_protocol::openai::chat::{ChatCompletionRequest, ChatCompletionResponse};
use ponyllm_protocol::openai::responses::{CreateResponseRequest, ResponseObject};
use ponyllm_protocol::translator::*;

#[derive(Debug, Clone)]
pub struct ProviderInfo {
    pub name: String,
    pub base_url: String,
    pub default_model: String,
    pub pool: Arc<KeyPool>,
}

#[derive(Debug, Clone)]
pub struct PonyGateway {
    pub providers: HashMap<String, ProviderInfo>,
    pub max_retries: usize,
}

impl PonyGateway {
    pub fn builder() -> PonyGatewayBuilder {
        PonyGatewayBuilder::default()
    }

    fn resolve_provider(&self, model: &str) -> Result<&ProviderInfo> {
        // 1. Exact match on default_model
        for info in self.providers.values() {
            if info.default_model == model {
                return Ok(info);
            }
        }

        // 2. Prefix matching e.g. "deepseek/deepseek-chat"
        if let Some((prefix, _)) = model.split_once('/') {
            if let Some(info) = self.providers.get(prefix) {
                return Ok(info);
            }
        }

        // 3. Keyword / model family heuristic matching
        let lower = model.to_lowercase();
        for (name, info) in &self.providers {
            if lower.contains(name)
                || (name == "openai" && (lower.starts_with("gpt") || lower.starts_with("o1") || lower.starts_with("o3")))
                || (name == "anthropic" && lower.starts_with("claude"))
                || (name == "deepseek" && lower.starts_with("deepseek"))
            {
                return Ok(info);
            }
        }

        // 4. Fallback to first registered provider
        self.providers
            .values()
            .next()
            .ok_or_else(|| CoreError::Internal("No providers registered in PonyGateway".to_string()))
    }

    /// In-memory Chat Completion API
    pub async fn chat_completion(&self, req: &ChatCompletionRequest) -> Result<ChatCompletionResponse> {
        let provider = self.resolve_provider(&req.model)?;
        let executor = UpstreamExecutor::new(provider.pool.clone(), self.max_retries);
        let is_anthropic_upstream = provider.base_url.ends_with("/anthropic")
            || provider.base_url.contains("api.anthropic.com")
            || provider.base_url.ends_with("/messages");

        if is_anthropic_upstream {
            let ant_req = chat_to_anthropic_request(req)?;
            let url = if provider.base_url.ends_with("/messages") {
                provider.base_url.clone()
            } else {
                format!("{}/v1/messages", provider.base_url.trim_end_matches('/'))
            };
            let body = serde_json::to_value(&ant_req)?;
            let resp_val = executor.execute_json_request(&url, &body).await?;
            let ant_resp: MessageResponse = serde_json::from_value(resp_val)?;
            let chat_resp = anthropic_to_chat_response(&ant_resp)?;
            Ok(chat_resp)
        } else {
            let url = format!("{}/v1/chat/completions", provider.base_url.trim_end_matches('/'));
            let body = serde_json::to_value(req)?;
            let resp_val = executor.execute_json_request(&url, &body).await?;
            let resp: ChatCompletionResponse = serde_json::from_value(resp_val)?;
            Ok(resp)
        }
    }

    /// In-memory Anthropic Messages API (with transparent bidirectional translation)
    pub async fn create_message(&self, req: &MessageRequest) -> Result<MessageResponse> {
        let provider = self.resolve_provider(&req.model)?;
        let executor = UpstreamExecutor::new(provider.pool.clone(), self.max_retries);
        let is_anthropic_upstream = provider.base_url.ends_with("/anthropic")
            || provider.base_url.contains("api.anthropic.com")
            || provider.base_url.ends_with("/messages");

        if is_anthropic_upstream {
            let url = if provider.base_url.ends_with("/messages") {
                provider.base_url.clone()
            } else {
                format!("{}/v1/messages", provider.base_url.trim_end_matches('/'))
            };
            let body = serde_json::to_value(req)?;
            let resp_val = executor.execute_json_request(&url, &body).await?;
            let ant_resp: MessageResponse = serde_json::from_value(resp_val)?;
            Ok(ant_resp)
        } else {
            let chat_req = anthropic_to_chat_request(req)?;
            let url = format!("{}/v1/chat/completions", provider.base_url.trim_end_matches('/'));
            let body = serde_json::to_value(&chat_req)?;
            let resp_val = executor.execute_json_request(&url, &body).await?;
            let chat_resp: ChatCompletionResponse = serde_json::from_value(resp_val)?;
            let ant_resp = chat_to_anthropic_response(&chat_resp)?;
            Ok(ant_resp)
        }
    }

    /// In-memory OpenAI Responses API
    pub async fn create_response(&self, req: &CreateResponseRequest) -> Result<ResponseObject> {
        let provider = self.resolve_provider(&req.model)?;
        let executor = UpstreamExecutor::new(provider.pool.clone(), self.max_retries);

        let url = format!("{}/v1/responses", provider.base_url.trim_end_matches('/'));
        let body = serde_json::to_value(req)?;

        let resp_val = executor.execute_json_request(&url, &body).await?;
        let resp: ResponseObject = serde_json::from_value(resp_val)?;
        Ok(resp)
    }
}

#[derive(Debug, Default)]
pub struct PonyGatewayBuilder {
    providers: HashMap<String, (String, String, RoutingStrategy)>,
    keys: Vec<(String, ApiKeyEntry)>,
    max_retries: usize,
}

impl PonyGatewayBuilder {
    pub fn add_provider(
        mut self,
        name: impl Into<String>,
        base_url: impl Into<String>,
        default_model: impl Into<String>,
        strategy: RoutingStrategy,
    ) -> Self {
        self.providers.insert(name.into(), (base_url.into(), default_model.into(), strategy));
        self
    }

    pub fn add_key(
        mut self,
        provider: impl Into<String>,
        id: impl Into<String>,
        api_key: impl Into<String>,
        priority: u32,
        weight: u32,
    ) -> Self {
        self.keys.push((provider.into(), ApiKeyEntry::new(id, api_key, priority, weight)));
        self
    }

    pub fn max_retries(mut self, retries: usize) -> Self {
        self.max_retries = retries;
        self
    }

    pub fn build(self) -> PonyGateway {
        let mut provider_infos = HashMap::new();
        let retries = if self.max_retries == 0 { 3 } else { self.max_retries };

        for (name, (base_url, default_model, strategy)) in self.providers {
            let pool = Arc::new(KeyPool::new(&name, strategy));
            for (p_name, key_entry) in &self.keys {
                if *p_name == name {
                    pool.add_key(ApiKeyEntry::new(
                        &key_entry.id,
                        &key_entry.api_key,
                        key_entry.priority,
                        key_entry.weight,
                    ));
                }
            }
            provider_infos.insert(
                name.clone(),
                ProviderInfo {
                    name,
                    base_url,
                    default_model,
                    pool,
                },
            );
        }

        PonyGateway {
            providers: provider_infos,
            max_retries: retries,
        }
    }
}
