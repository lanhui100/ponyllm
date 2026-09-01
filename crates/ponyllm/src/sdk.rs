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

    fn resolve_provider(&self, _model: &str) -> Result<&ProviderInfo> {
        self.providers
            .values()
            .next()
            .ok_or_else(|| CoreError::Internal("No providers registered in PonyGateway".to_string()))
    }

    /// In-memory Chat Completion API
    pub async fn chat_completion(&self, req: &ChatCompletionRequest) -> Result<ChatCompletionResponse> {
        let provider = self.resolve_provider(&req.model)?;
        let executor = UpstreamExecutor::new(provider.pool.clone(), self.max_retries);
        let url = format!("{}/v1/chat/completions", provider.base_url.trim_end_matches('/'));

        let body = serde_json::to_value(req)?;
        let resp_val = executor.execute_json_request(&url, &body).await?;
        let resp: ChatCompletionResponse = serde_json::from_value(resp_val)?;
        Ok(resp)
    }

    /// In-memory Anthropic Messages API (with transparent bidirectional translation)
    pub async fn create_message(&self, req: &MessageRequest) -> Result<MessageResponse> {
        let provider = self.resolve_provider(&req.model)?;
        let executor = UpstreamExecutor::new(provider.pool.clone(), self.max_retries);

        let chat_req = anthropic_to_chat_request(req)?;
        let url = format!("{}/v1/chat/completions", provider.base_url.trim_end_matches('/'));
        let body = serde_json::to_value(&chat_req)?;

        let resp_val = executor.execute_json_request(&url, &body).await?;
        let chat_resp: ChatCompletionResponse = serde_json::from_value(resp_val)?;
        let ant_resp = chat_to_anthropic_response(&chat_resp)?;
        Ok(ant_resp)
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
