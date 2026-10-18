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
    } else if lower == "laya" || lower == "laya-free" {
        "convaiinnovations/laya".to_string()
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

/// Strips the cosmetic free-tier suffix (`-free` / `:free`) from a model name.
///
/// Free-tier upstreams publish the same logical model under `<base>-free`
/// (opencode-zen style) or `<base>:free` (OpenRouter style). The suffix carries
/// no routing semantics, so bare-name matching compares names with it removed.
/// Returns `model` unchanged when there is no free suffix. ASCII case-insensitive.
pub fn strip_free_suffix(model: &str) -> &str {
    match model.get(model.len().saturating_sub(5)..) {
        Some(tail)
            if model.len() > 5
                && (tail.eq_ignore_ascii_case("-free") || tail.eq_ignore_ascii_case(":free")) =>
        {
            &model[..model.len() - 5]
        }
        _ => model,
    }
}

/// Whether two model names denote the same logical model modulo the cosmetic
/// free-tier suffix (`-free` / `:free`). ASCII case-insensitive.
pub fn same_model_modulo_free_suffix(a: &str, b: &str) -> bool {
    strip_free_suffix(a).eq_ignore_ascii_case(strip_free_suffix(b))
}

/// Client-facing aliases exposed in `/v1/models` for a canonical model name.
pub fn model_aliases(canonical: &str) -> &'static [&'static str] {
    if canonical.eq_ignore_ascii_case("deepseek-flash") {
        &["deepseek-v4.1-flash"]
    } else if canonical.eq_ignore_ascii_case("convaiinnovations/laya") {
        &["laya", "laya-free"]
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

    #[test]
    fn test_strip_free_suffix_truth_table() {
        assert_eq!(strip_free_suffix("glm-5.3-flash-free"), "glm-5.3-flash");
        assert_eq!(strip_free_suffix("glm-5.3-flash:FREE"), "glm-5.3-flash");
        assert_eq!(
            strip_free_suffix("muse-spark-1.3-contributor-free"),
            "muse-spark-1.3-contributor"
        );
        // Names that are not free-tier variants stay untouched.
        assert_eq!(strip_free_suffix("glm-5.3-flash"), "glm-5.3-flash");
        assert_eq!(strip_free_suffix("free"), "free");
        assert_eq!(strip_free_suffix("-free"), "-free");
        assert_eq!(
            strip_free_suffix("glm-5.3-flash-freely"),
            "glm-5.3-flash-freely"
        );
        assert_eq!(
            strip_free_suffix("glm-5.3-flash:freestyle"),
            "glm-5.3-flash:freestyle"
        );
        // Multi-byte input must not panic on a non-char-boundary slice.
        assert_eq!(strip_free_suffix("模型-free"), "模型");
        assert_eq!(strip_free_suffix("模型"), "模型");
    }

    #[test]
    fn test_same_model_modulo_free_suffix() {
        assert!(same_model_modulo_free_suffix(
            "glm-5.3-flash",
            "glm-5.3-flash-free"
        ));
        assert!(same_model_modulo_free_suffix(
            "glm-5.3-flash",
            "glm-5.3-flash:free"
        ));
        assert!(same_model_modulo_free_suffix(
            "GLM-5.3-Flash-FREE",
            "glm-5.3-flash"
        ));
        assert!(!same_model_modulo_free_suffix(
            "glm-5.3-flash",
            "glm-5.3-flash-premium"
        ));
        assert!(!same_model_modulo_free_suffix("glm-5.3", "glm-5.3-flash"));
    }

    #[test]
    fn test_laya_alias_canonicalizes() {
        assert_eq!(
            canonicalize_model_name("laya"),
            "convaiinnovations/laya"
        );
        assert_eq!(
            canonicalize_model_name("Laya"),
            "convaiinnovations/laya"
        );
        assert_eq!(
            canonicalize_model_name("laya-free"),
            "convaiinnovations/laya"
        );
        assert_eq!(
            canonicalize_model_name("convaiinnovations/laya"),
            "convaiinnovations/laya"
        );
        assert!(model_aliases("convaiinnovations/laya").contains(&"laya"));
    }
}
