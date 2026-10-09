//! Upstream endpoint URL normalization shared by the gateway and the embedded SDK.
//!
//! Each function accepts either a service root (`https://api.example.com`),
//! a versioned root (`.../v1`), or an already-complete endpoint path, and
//! always returns the complete endpoint URL without double `/v1` segments.

/// Normalize to `<base>/v1/chat/completions`.
pub fn normalize_chat_completions_url(base_url: &str) -> String {
    normalize_endpoint_url(base_url, "chat/completions")
}

/// Normalize to `<base>/v1/responses`.
pub fn normalize_responses_url(base_url: &str) -> String {
    normalize_endpoint_url(base_url, "responses")
}

/// Normalize to `<base>/v1/messages`.
pub fn normalize_messages_url(base_url: &str) -> String {
    normalize_endpoint_url(base_url, "messages")
}

/// Normalize to `<base>/systemone` (zen-jev 透传路由).
///
/// 与其它协议不同:systemone 上游路径不带 `/v1` 前缀,base 去尾 slash 后
/// 直接拼接 `/systemone`;已是 `/systemone` 结尾则原样返回.
pub fn normalize_systemone_url(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.ends_with("/systemone") {
        return trimmed.to_string();
    }
    format!("{}/systemone", trimmed)
}

/// Canonical upstream model name for known retired/renamed aliases.
///
/// DeepSeek's live API name is `deepseek-flash` (`deepseek-v4.1-flash` was
/// never a valid upstream name and is rejected with 400).
/// Gemini 3's suffix models (`gemini-3.8-flash-high`, `-medium`, `-low`, `-tiered`)
/// are internal variants that route to `gemini-3.8-flash`.
pub fn canonicalize_model_name(model: &str) -> String {
    let lower = model.to_ascii_lowercase();
    if lower == "deepseek-v4.1-flash" {
        "deepseek-flash".to_string()
    } else if lower == "gemini-3.8-flash-high"
        || lower == "gemini-3.8-flash-medium"
        || lower == "gemini-3.8-flash-low"
        || lower == "gemini-3.8-flash-tiered"
    {
        "gemini-3.8-flash".to_string()
    } else {
        model.to_string()
    }
}

/// Client-facing aliases exposed in `/v1/models` for a canonical model name.
pub fn model_aliases(canonical: &str) -> &'static [&'static str] {
    if canonical.eq_ignore_ascii_case("deepseek-flash") {
        &["deepseek-v4.1-flash"]
    } else {
        &[]
    }
}

fn normalize_endpoint_url(base_url: &str, leaf: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    let suffix = format!("/{}", leaf);
    if trimmed.ends_with(&suffix) {
        return trimmed.to_string();
    }
    if trimmed.ends_with("/v1") {
        return format!("{}/{}", trimmed, leaf);
    }
    // Anthropic-style versioned roots such as `.../anthropic` already carry
    // their final path segment; only append the missing `/v1/<leaf>`.
    format!("{}/v1/{}", trimmed, leaf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_roots_and_full_paths() {
        assert_eq!(
            normalize_chat_completions_url("https://api.openai.com"),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(
            normalize_chat_completions_url("https://api.openai.com/v1"),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(
            normalize_chat_completions_url("https://api.openai.com/v1/chat/completions"),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(
            normalize_responses_url("https://api.openai.com/v1"),
            "https://api.openai.com/v1/responses"
        );
        assert_eq!(
            normalize_responses_url("https://api.openai.com/v1/responses"),
            "https://api.openai.com/v1/responses"
        );
        assert_eq!(
            normalize_responses_url("https://resp.example.com/responses"),
            "https://resp.example.com/responses"
        );
        assert_eq!(
            normalize_messages_url("https://api.deepseek.com/anthropic"),
            "https://api.deepseek.com/anthropic/v1/messages"
        );
        assert_eq!(
            normalize_messages_url("https://x.example.com/v1/messages"),
            "https://x.example.com/v1/messages"
        );
    }

    #[test]
    fn test_normalize_systemone() {
        assert_eq!(
            normalize_systemone_url("https://zen.example.com/opencode/zen/v1"),
            "https://zen.example.com/opencode/zen/v1/systemone"
        );
        assert_eq!(
            normalize_systemone_url("https://zen.example.com/opencode/zen/v1/"),
            "https://zen.example.com/opencode/zen/v1/systemone"
        );
        assert_eq!(
            normalize_systemone_url("https://zen.example.com/opencode/zen/v1/systemone"),
            "https://zen.example.com/opencode/zen/v1/systemone"
        );
    }

    #[test]
    fn test_deepseek_v41_flash_alias_canonicalizes() {
        assert_eq!(
            canonicalize_model_name("deepseek-v4.1-flash"),
            "deepseek-flash"
        );
        assert_eq!(
            canonicalize_model_name("DeepSeek-V4.1-Flash"),
            "deepseek-flash"
        );
        assert_eq!(canonicalize_model_name("deepseek-flash"), "deepseek-flash");
        assert_eq!(canonicalize_model_name("deepseek-chat"), "deepseek-chat");
        assert!(model_aliases("deepseek-flash").contains(&"deepseek-v4.1-flash"));
        assert!(model_aliases("deepseek-chat").is_empty());
    }
}
