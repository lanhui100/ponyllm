use std::collections::HashMap;
use std::sync::Arc;
use ponyllm_core::error::{CoreError, Result};
use ponyllm_core::executor::UpstreamExecutor;
use ponyllm_core::pool::{ApiKeyEntry, KeyPool, RoutingStrategy, UpstreamProtocol};
use ponyllm_core::{canonicalize_model_name, normalize_chat_completions_url, normalize_gemini_url, normalize_messages_url, normalize_responses_url};
use ponyllm_protocol::anthropic::messages::{MessageRequest, MessageResponse};
use ponyllm_protocol::openai::chat::{ChatCompletionRequest, ChatCompletionResponse};
use ponyllm_protocol::openai::responses::{CreateResponseRequest, ResponseObject};
use ponyllm_protocol::sanitize::{sanitize_extra, TargetProtocol};
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

    /// Native Gemini `generateContent` endpoint for `model` (non-streaming).
    ///
    /// Gemini's version root is `/v1beta`, so the OpenAI-shaped normalizers
    /// above do not apply; this delegates to the shared
    /// [`ponyllm_core::normalize_gemini_url`] so the SDK and the HTTP gateway
    /// dial byte-identical URLs (including the `models/` prefix and trailing
    /// `/models` base tolerance).
    fn gemini_url(&self, model: &str) -> String {
        normalize_gemini_url(&self.base_url, model, false)
    }

    fn antigravity_url(&self) -> String {
        format!("{}/v1internal:generateContent", self.base_url.trim_end_matches('/'))
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
        // Retired/renamed aliases resolve to the live upstream name first.
        let canonical = canonicalize_model_name(model);
        // Deterministic name order so multi-provider matches never depend on
        // HashMap iteration randomness.
        let mut names: Vec<&String> = self.providers.keys().collect();
        names.sort();

        // 1. Exact match on default_model or configured models list
        // (alias never shadows an explicit config entry).
        for name in &names {
            let info = &self.providers[*name];
            if info.default_model == model || info.models.iter().any(|m| m == model) {
                return Ok(info);
            }
        }

        // 1b. Canonical alias fallback (e.g. deepseek-v4.1-flash -> deepseek-flash).
        if canonical != model {
            for name in &names {
                let info = &self.providers[*name];
                if info.default_model == canonical || info.models.iter().any(|m| m == &canonical) {
                    return Ok(info);
                }
            }
        }

        // 2. Prefix matching e.g. "deepseek/deepseek-chat"
        if let Some((prefix, _)) = canonical.split_once('/') {
            if let Some(info) = self.providers.get(prefix) {
                return Ok(info);
            }
        }

        // 3. Keyword / model family heuristic matching
        let lower = canonical.to_lowercase();
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
        let canonical = canonicalize_model_name(&req.model);
        // Upstream wire model is always canonical; translators copy req.model.
        let mut wire_req = req.clone();
        wire_req.model = canonical.clone();
        let req = &wire_req;
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
                // F2 (contract v5): the embedded SDK serializes same-protocol
                // requests directly, so the flattened `extra` map must be
                // sanitized here too — otherwise the HTTP gateway's reserved-key
                // protection silently does not apply on this path.
                let mut wire_body = req.clone();
                wire_body.extra = sanitize_extra(
                    std::mem::take(&mut wire_body.extra),
                    TargetProtocol::OpenAiChat,
                );
                let body = serde_json::to_value(&wire_body)?;
                let resp_val = executor.execute_json_request(&provider.chat_url(), &body).await?;
                let resp: ChatCompletionResponse = serde_json::from_value(resp_val)?;
                Ok(resp)
            }
            UpstreamProtocol::Gemini => {
                // Chat is the single canonical pivot: the request is already
                // Chat, so only the Chat → native Gemini hop applies.
                let body = chat_to_gemini_request(req, &canonical, req.get_reasoning_effort())?;
                let resp_val = executor
                    .execute_json_request(&provider.gemini_url(&canonical), &body)
                    .await?;
                let chat_val = gemini_to_chat_response(&resp_val, &canonical);
                let resp: ChatCompletionResponse = serde_json::from_value(chat_val)?;
                Ok(resp)
            }
            UpstreamProtocol::Systemone => return Err(CoreError::Internal("Systemone protocol requires the /systemone API".to_string())),
            UpstreamProtocol::Antigravity => {
                let ant_req = chat_to_antigravity_request(req, &canonical, "aicode-consumers", req.get_reasoning_effort(), "")?;
                let resp_val = executor.execute_json_request(&provider.antigravity_url(), &ant_req).await?;
                let chat_val = antigravity_to_chat_response(&resp_val, &canonical);
                let resp: ChatCompletionResponse = serde_json::from_value(chat_val)?;
                Ok(resp)
            }
        }
    }

    /// In-memory Anthropic Messages API (with transparent bidirectional translation)
    pub async fn create_message(&self, req: &MessageRequest) -> Result<MessageResponse> {
        let provider = self.resolve_provider(&req.model)?;
        let canonical = canonicalize_model_name(&req.model);
        let mut wire_req = req.clone();
        wire_req.model = canonical.clone();
        let req = &wire_req;
        let executor = UpstreamExecutor::with_client(provider.pool.clone(), self.http_client.clone(), self.max_retries);

        match provider.native_protocol() {
            UpstreamProtocol::Anthropic => {
                // F2 (contract v5): same-protocol boundary — sanitize before the
                // bare `to_value`, see the Chat branch above.
                let mut wire_body = req.clone();
                wire_body.extra = sanitize_extra(
                    std::mem::take(&mut wire_body.extra),
                    TargetProtocol::Anthropic,
                );
                let body = serde_json::to_value(&wire_body)?;
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
            UpstreamProtocol::Gemini => {
                // Chat is the pivot: Messages → Chat (existing translator) then
                // Chat → native Gemini; the response comes back as Chat and is
                // projected to Messages.
                let chat_req = anthropic_to_chat_request(req)?;
                let body =
                    chat_to_gemini_request(&chat_req, &canonical, chat_req.get_reasoning_effort())?;
                let resp_val = executor
                    .execute_json_request(&provider.gemini_url(&canonical), &body)
                    .await?;
                let chat_val = gemini_to_chat_response(&resp_val, &canonical);
                let chat_resp: ChatCompletionResponse = serde_json::from_value(chat_val)?;
                let ant_resp = chat_to_anthropic_response(&chat_resp)?;
                Ok(ant_resp)
            }
            UpstreamProtocol::Systemone => return Err(CoreError::Internal("Systemone protocol requires the /systemone API".to_string())),
            UpstreamProtocol::Antigravity => {
                let ant_req = messages_to_antigravity_request(req, &canonical, "aicode-consumers", req.get_reasoning_effort(), "")?;
                let resp_val = executor.execute_json_request(&provider.antigravity_url(), &ant_req).await?;
                let msg_val = antigravity_to_messages_response(&resp_val, &canonical);
                let resp: MessageResponse = serde_json::from_value(msg_val)?;
                Ok(resp)
            }
        }
    }

    /// In-memory OpenAI Responses API
    pub async fn create_response(&self, req: &CreateResponseRequest) -> Result<ResponseObject> {
        let provider = self.resolve_provider(&req.model)?;
        let canonical = canonicalize_model_name(&req.model);
        let mut wire_req = req.clone();
        wire_req.model = canonical.clone();
        let req = &wire_req;
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
                // F2 (contract v5): same-protocol boundary — sanitize before the
                // bare `to_value`, see the Chat branch above.
                let mut wire_body = req.clone();
                wire_body.extra = sanitize_extra(
                    std::mem::take(&mut wire_body.extra),
                    TargetProtocol::OpenAiResponses,
                );
                let body = serde_json::to_value(&wire_body)?;
                let resp_val = executor.execute_json_request(&provider.responses_url(), &body).await?;
                let resp: ResponseObject = serde_json::from_value(resp_val)?;
                Ok(resp)
            }
            UpstreamProtocol::Gemini => {
                // Chat is the pivot: Responses → Chat (existing translator)
                // then Chat → native Gemini; the response comes back as Chat
                // and is projected to the Responses object.
                let chat_req = responses_to_chat_request(req)?;
                let body =
                    chat_to_gemini_request(&chat_req, &canonical, chat_req.get_reasoning_effort())?;
                let resp_val = executor
                    .execute_json_request(&provider.gemini_url(&canonical), &body)
                    .await?;
                let chat_val = gemini_to_chat_response(&resp_val, &canonical);
                let chat_resp: ChatCompletionResponse = serde_json::from_value(chat_val)?;
                let resp_obj = chat_to_responses_response(&chat_resp)?;
                Ok(resp_obj)
            }
            UpstreamProtocol::Systemone => Err(CoreError::Internal("Systemone protocol requires the /systemone API".to_string())),
            UpstreamProtocol::Antigravity => {
                Err(CoreError::Internal("Antigravity protocol does not support Responses API".to_string()))
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

#[cfg(test)]
mod sdk_gemini_wiring_tests {
    use super::*;

    fn gemini_provider(base_url: &str) -> ProviderInfo {
        ProviderInfo {
            name: "gemini".to_string(),
            base_url: base_url.to_string(),
            default_model: "gemini-3.8-flash".to_string(),
            models: vec![],
            pool: Arc::new(KeyPool::new("gemini", RoutingStrategy::Priority)),
            default_protocol: Some(UpstreamProtocol::Gemini),
        }
    }

    #[test]
    fn gemini_url_matches_gateway_normalizer() {
        // The embedded SDK must dial the byte-identical URL the HTTP gateway
        // builds, otherwise the two surfaces silently diverge.
        let p = gemini_provider("https://generativelanguage.googleapis.com/v1beta");
        assert_eq!(
            p.gemini_url("gemini-3.8-flash"),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-3.8-flash:generateContent"
        );
        // Trailing slash on the base and an already-prefixed `models/` model
        // are both tolerated (operator/stdlib shapes).
        let p = gemini_provider("https://generativelanguage.googleapis.com/v1beta/");
        assert_eq!(
            p.gemini_url("models/gemini-3.8-flash"),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-3.8-flash:generateContent"
        );
        // Non-streaming only: generateContent, never streamGenerateContent.
        assert!(!p.gemini_url("gemini-3.8-flash").contains("streamGenerateContent"));
    }

    #[test]
    fn gemini_protocol_routes_to_native_branch() {
        // `default_protocol = "gemini"` must win over the URL heuristic, so the
        // SDK dispatches to the Gemini branch instead of OpenAI Chat.
        let p = gemini_provider("https://generativelanguage.googleapis.com/v1beta");
        assert_eq!(p.native_protocol(), UpstreamProtocol::Gemini);

        // An unset protocol keeps the legacy heuristic untouched.
        let mut legacy = gemini_provider("https://api.openai.com");
        legacy.default_protocol = None;
        assert_eq!(legacy.native_protocol(), UpstreamProtocol::Chat);
        legacy.base_url = "https://api.anthropic.com".to_string();
        assert_eq!(legacy.native_protocol(), UpstreamProtocol::Anthropic);
    }
}
