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
}
