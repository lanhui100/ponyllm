use std::collections::HashMap;
use std::sync::Arc;
use ponyllm_core::error::{CoreError, Result};
use ponyllm_core::executor::UpstreamExecutor;
use ponyllm_core::pool::{ApiKeyEntry, KeyPool, RoutingStrategy, UpstreamProtocol};
use ponyllm_core::{normalize_chat_completions_url, normalize_messages_url, normalize_responses_url};
use ponyllm_protocol::anthropic::messages::{MessageRequest, MessageResponse};
use ponyllm_protocol::openai::chat::{ChatCompletionRequest, ChatCompletionResponse};
use ponyllm_protocol::openai::responses::{CreateResponseRequest, ResponseObject};
use ponyllm_protocol::translator::*;

#[derive(Debug, Clone)]
pub struct ProviderInfo {
    pub name: String,
    pub base_url: String,
    pub default_model: String,
    pub models: Vec<String>,
    pub pool: Arc<KeyPool>,
    pub default_protocol: Option<UpstreamProtocol>,
}

impl ProviderInfo {
    fn native_protocol(&self) -> UpstreamProtocol {
        if let Some(p) = self.default_protocol {
            return p;
        }
        let trimmed = self.base_url.trim_end_matches('/');
        if trimmed.ends_with("/anthropic")
            || trimmed.contains("api.anthropic.com")
            || trimmed.ends_with("/messages")
        {
            UpstreamProtocol::Anthropic
        } else {
            UpstreamProtocol::Chat
        }
    }

    fn messages_url(&self) -> String {
        normalize_messages_url(&self.base_url)
    }

    fn chat_url(&self) -> String {
        normalize_chat_completions_url(&self.base_url)
    }

    fn responses_url(&self) -> String {
        normalize_responses_url(&self.base_url)
    }
}

#[derive(Debug, Clone)]
pub struct PonyGateway {
    pub providers: HashMap<String, ProviderInfo>,
    pub max_retries: usize,
    pub http_client: reqwest::Client,
}

impl PonyGateway {
    pub fn builder() -> PonyGatewayBuilder {
        PonyGatewayBuilder::default()
    }

    /// Return all configured model IDs across all providers: Vec<(model_id, provider_name)>
    /// Provider iteration is name-sorted so listing order is deterministic.
    pub fn list_models(&self) -> Vec<(String, String)> {
        let mut result = Vec::new();
        let mut names: Vec<&String> = self.providers.keys().collect();
        names.sort();
        for provider_name in names {
            let info = &self.providers[provider_name];
            if !info.default_model.is_empty() {
                result.push((info.default_model.clone(), provider_name.clone()));
            }
            for m in &info.models {
                if m != &info.default_model && !result.iter().any(|(existing_m, _)| existing_m == m) {
                    result.push((m.clone(), provider_name.clone()));
                }
            }
        }
        result
    }

    fn resolve_provider(&self, model: &str) -> Result<&ProviderInfo> {
        // Deterministic name order so multi-provider matches never depend on
        // HashMap iteration randomness.
        let mut names: Vec<&String> = self.providers.keys().collect();
        names.sort();

        // 1. Exact match on default_model or configured models list
        for name in &names {
            let info = &self.providers[*name];
            if info.default_model == model || info.models.iter().any(|m| m == model) {
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
        for name in &names {
            let info = &self.providers[*name];
            if lower.contains(name.as_str())
                || (*name == "openai" && (lower.starts_with("gpt") || lower.starts_with("o1") || lower.starts_with("o3")))
                || (*name == "anthropic" && lower.starts_with("claude"))
                || (*name == "deepseek" && lower.starts_with("deepseek"))
            {
                return Ok(info);
            }
        }

        // 4. No random fallback: unknown models are a caller error, never
        // silently routed to an arbitrary provider.
        Err(CoreError::Internal(format!(
            "No provider configured to handle model '{}'",
            model
        )))
    }

    /// In-memory Chat Completion API
    pub async fn chat_completion(&self, req: &ChatCompletionRequest) -> Result<ChatCompletionResponse> {
        let provider = self.resolve_provider(&req.model)?;
        let executor = UpstreamExecutor::with_client(provider.pool.clone(), self.http_client.clone(), self.max_retries);

        match provider.native_protocol() {
            UpstreamProtocol::Anthropic => {
                let ant_req = chat_to_anthropic_request(req)?;
                let body = serde_json::to_value(&ant_req)?;
                let resp_val = executor.execute_json_request(&provider.messages_url(), &body).await?;
                let ant_resp: MessageResponse = serde_json::from_value(resp_val)?;
                let chat_resp = anthropic_to_chat_response(&ant_resp)?;
                Ok(chat_resp)
            }
            UpstreamProtocol::Responses => {
                let resp_req = chat_to_responses_request(req)?;
                let body = serde_json::to_value(&resp_req)?;
                let resp_val = executor.execute_json_request(&provider.responses_url(), &body).await?;
                let resp_obj: ResponseObject = serde_json::from_value(resp_val)?;
                let chat_resp = responses_to_chat_response(&resp_obj)?;
                Ok(chat_resp)
            }
            UpstreamProtocol::Chat => {
                let body = serde_json::to_value(req)?;
                let resp_val = executor.execute_json_request(&provider.chat_url(), &body).await?;
                let resp: ChatCompletionResponse = serde_json::from_value(resp_val)?;
                Ok(resp)
            }
        }
    }

    /// In-memory Anthropic Messages API (with transparent bidirectional translation)
    pub async fn create_message(&self, req: &MessageRequest) -> Result<MessageResponse> {
        let provider = self.resolve_provider(&req.model)?;
        let executor = UpstreamExecutor::with_client(provider.pool.clone(), self.http_client.clone(), self.max_retries);

        match provider.native_protocol() {
            UpstreamProtocol::Anthropic => {
                let body = serde_json::to_value(req)?;
                let resp_val = executor.execute_json_request(&provider.messages_url(), &body).await?;
                let ant_resp: MessageResponse = serde_json::from_value(resp_val)?;
                Ok(ant_resp)
            }
            UpstreamProtocol::Responses => {
                let resp_req = anthropic_to_responses_request(req)?;
                let body = serde_json::to_value(&resp_req)?;
                let resp_val = executor.execute_json_request(&provider.responses_url(), &body).await?;
                let resp_obj: ResponseObject = serde_json::from_value(resp_val)?;
                let ant_resp = responses_to_anthropic_response(&resp_obj)?;
                Ok(ant_resp)
            }
            UpstreamProtocol::Chat => {
                let chat_req = anthropic_to_chat_request(req)?;
                let body = serde_json::to_value(&chat_req)?;
                let resp_val = executor.execute_json_request(&provider.chat_url(), &body).await?;
                let chat_resp: ChatCompletionResponse = serde_json::from_value(resp_val)?;
                let ant_resp = chat_to_anthropic_response(&chat_resp)?;
                Ok(ant_resp)
            }
        }
    }

    /// In-memory OpenAI Responses API
    pub async fn create_response(&self, req: &CreateResponseRequest) -> Result<ResponseObject> {
        let provider = self.resolve_provider(&req.model)?;
        let executor = UpstreamExecutor::with_client(provider.pool.clone(), self.http_client.clone(), self.max_retries);

        match provider.native_protocol() {
            UpstreamProtocol::Anthropic => {
                let ant_req = responses_to_anthropic_request(req)?;
                let body = serde_json::to_value(&ant_req)?;
                let resp_val = executor.execute_json_request(&provider.messages_url(), &body).await?;
                let ant_resp: MessageResponse = serde_json::from_value(resp_val)?;
                let resp_obj = anthropic_to_responses_response(&ant_resp)?;
                Ok(resp_obj)
            }
            UpstreamProtocol::Chat => {
                let chat_req = responses_to_chat_request(req)?;
                let body = serde_json::to_value(&chat_req)?;
                let resp_val = executor.execute_json_request(&provider.chat_url(), &body).await?;
                let chat_resp: ChatCompletionResponse = serde_json::from_value(resp_val)?;
                let resp_obj = chat_to_responses_response(&chat_resp)?;
                Ok(resp_obj)
            }
            UpstreamProtocol::Responses => {
                let body = serde_json::to_value(req)?;
                let resp_val = executor.execute_json_request(&provider.responses_url(), &body).await?;
                let resp: ResponseObject = serde_json::from_value(resp_val)?;
                Ok(resp)
            }
        }
    }
}

#[derive(Debug, Default)]
pub struct PonyGatewayBuilder {
    providers: HashMap<String, (String, String, RoutingStrategy, Option<UpstreamProtocol>)>,
    models: HashMap<String, Vec<String>>,
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
        self.providers
            .insert(name.into(), (base_url.into(), default_model.into(), strategy, None));
        self
    }

    /// Declare the native wire protocol for a previously added provider.
    /// Must be called after [`Self::add_provider`]; unknown names are ignored.
    pub fn set_provider_protocol(
        mut self,
        name: impl Into<String>,
        protocol: UpstreamProtocol,
    ) -> Self {
        if let Some(entry) = self.providers.get_mut(&name.into()) {
            entry.3 = Some(protocol);
        }
        self
    }

    pub fn add_model(
        mut self,
        provider: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        self.models.entry(provider.into()).or_default().push(model.into());
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

        for (name, (base_url, default_model, strategy, default_protocol)) in self.providers {
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
            let extra_models = self.models.get(&name).cloned().unwrap_or_default();
            provider_infos.insert(
                name.clone(),
                ProviderInfo {
                    name,
                    base_url,
                    default_model,
                    models: extra_models,
                    pool,
                    default_protocol,
                },
            );
        }

        PonyGateway {
            providers: provider_infos,
            max_retries: retries,
            http_client: ponyllm_core::executor::create_upstream_http_client(),
        }
    }
}
