//! FROZEN RED-PHASE ACCEPTANCE — Wave-2b (contract v6, review-driven).
//!
//! Three egress items:
//! 1. **socks full-chain convergence**: the provider/model `proxy` field is
//!    validated by `ponyllm_server::egress::check_proxy_url_fast` (admin write
//!    path, NOT the egress-pool entry path). `socks5`/`socks5h` (any case) must
//!    be refused there too, the message must name the socks scheme, and it must
//!    never echo proxy credentials.
//! 2. **NAT64 / IPv4-translated families**: `64:ff9b::/96`, `64:ff9b:1::/48`
//!    and `::ffff:0:0/96` (SIIT IPv4-translated) embed an IPv4 that the dialer
//!    will reach; every one of the four gates (proxy / probe / data-plane /
//!    config) must judge the EMBEDDED address, while `64:ff9b::808:808`
//!    (8.8.8.8) stays legal.
//! 3. **parser parity**: `check_proxy_url_fast` uses a hand-rolled splitter
//!    while the config gate uses the WHATWG `url()` parser — the same input
//!    must get the same verdict (RED today: the fast path ALLOWs what the
//!    config path BLOCKs).

use ponyllm_config::validate_egress_entry;
use ponyllm_server::egress::{check_data_plane_url, check_probe_url_fast, check_proxy_url_fast};

// ---------------------------------------------------------------------------
// 1. socks scheme convergence on the provider/model `proxy` path
// ---------------------------------------------------------------------------

/// The provider/model `proxy` field enters through
/// `check_proxy_url_fast` (see crates/ponyllm-server/src/routes/admin.rs
/// L1745 / L2032 / L2418 / L2777 / L5636). The workspace `reqwest` has no
/// `socks` feature, so a socks proxy cannot be honoured — accepting it means
/// a silent DIRECT fallback (egress de-coupling).
#[test]
fn v6_proxy_guard_rejects_socks_schemes_including_uppercase() {
    for entry in [
        "socks5://127.0.0.1:1080",  // local pproxy shape, must still be refused
        "socks5h://127.0.0.1:1080", // remote-DNS variant
        "SOCKS5://127.0.0.1:1080",  // uppercase scheme
        "SOCKS5H://127.0.0.1:1080", // uppercase + h
        "socks5://egress-1.example.com:1080", // public target
    ] {
        let err = check_proxy_url_fast(entry).err().unwrap_or_else(|| {
            panic!("v6: proxy guard must refuse '{entry}' (no reqwest socks feature), but it was accepted")
        });
        assert!(
            err.to_ascii_lowercase().contains("socks"),
            "v6: refusal for '{entry}' must name the socks scheme, got '{err}'"
        );
    }
}

/// Credential redaction: a socks URL carrying `user:secret` must be refused
/// WITHOUT echoing the secret back (errors surface in admin API responses and
/// gateway logs).
#[test]
fn v6_socks_refusal_never_echoes_credentials() {
    let err = check_proxy_url_fast("socks5://user:secret@127.0.0.1:1080")
        .err()
        .expect("v6: socks proxy with credentials must be refused");
    assert!(
        !err.contains("secret"),
        "v6: refusal message must not echo the proxy password, got '{err}'"
    );
    assert!(
        !err.contains("user:secret"),
        "v6: refusal message must not echo the userinfo, got '{err}'"
    );
    assert!(
        err.to_ascii_lowercase().contains("socks"),
        "v6: refusal must still name the socks scheme, got '{err}'"
    );
}

/// Over-tightening guard: plain http(s) proxies (with or without userinfo)
/// stay legal on the same path.
#[test]
fn v6_http_proxies_stay_legal_on_the_proxy_path() {
    for entry in [
        "http://127.0.0.1:8899",
        "http://localhost:7890",
        "https://egress-1.example.com:443",
        "http://user:pass@egress.example.com:8899",
    ] {
        check_proxy_url_fast(entry).unwrap_or_else(|e| {
            panic!("v6: '{entry}' must stay legal on the proxy path, got '{e}'")
        });
    }
}

// ---------------------------------------------------------------------------
// 2. NAT64 / IPv4-translated families on all four gates
// ---------------------------------------------------------------------------

/// `(url, why)` — every entry embeds a private or metadata IPv4.
const BLOCKED_EMBEDDED_V6: &[(&str, &str)] = &[
    (
        "http://[64:ff9b::a9fe:a9fe]:8899",
        "NAT64 64:ff9b::/96 embedding 169.254.169.254",
    ),
    (
        "http://[64:ff9b::10.0.0.5]:8899",
        "NAT64 64:ff9b::/96 embedding 10.0.0.5",
    ),
    (
        "http://[64:ff9b:1::a9fe:a9fe]:8899",
        "local-use NAT64 64:ff9b:1::/48 embedding 169.254.169.254",
    ),
    (
        "http://[::ffff:0:a9fe:a9fe]:8899",
        "IPv4-translated ::ffff:0:0/96 embedding 169.254.169.254",
    ),
];

/// `64:ff9b::808:808` embeds 8.8.8.8 — a real public resolver, so blocking it
/// would be over-tightening.
const ALLOWED_EMBEDDED_V6: &[(&str, &str)] = &[(
    "http://[64:ff9b::808:808]:8899",
    "NAT64 64:ff9b::/96 embedding public 8.8.8.8",
)];

#[test]
fn v6_proxy_and_config_gates_block_nat64_families() {
    for (url, why) in BLOCKED_EMBEDDED_V6 {
        let proxy_err = check_proxy_url_fast(url).err().unwrap_or_else(|| {
            panic!("v6: proxy guard must refuse {why} ({url}), but it was accepted")
        });
        assert!(
            proxy_err.contains("blocked"),
            "v6: proxy refusal for {why} must explain the blocked address, got '{proxy_err}'"
        );
        let config_err = validate_egress_entry(url).err().unwrap_or_else(|| {
            panic!("v6: config gate must refuse {why} ({url}), but it was accepted")
        });
        let lower = config_err.to_ascii_lowercase();
        assert!(
            lower.contains("blocked") || lower.contains("invalid"),
            "v6: config refusal for {why} must explain the refusal, got '{config_err}'"
        );
    }

    for (url, why) in ALLOWED_EMBEDDED_V6 {
        check_proxy_url_fast(url)
            .unwrap_or_else(|e| panic!("v6: proxy guard must allow {why} ({url}), got '{e}'"));
        validate_egress_entry(url)
            .unwrap_or_else(|e| panic!("v6: config gate must allow {why} ({url}), got '{e}'"));
    }
}

#[test]
fn v6_probe_gate_blocks_nat64_families() {
    for (url, why) in BLOCKED_EMBEDDED_V6 {
        let err = check_probe_url_fast(url).err().unwrap_or_else(|| {
            panic!("v6: probe guard must refuse {why} ({url}), but it was accepted")
        });
        assert!(
            err.contains("blocked"),
            "v6: probe refusal for {why} must explain the blocked address, got '{err}'"
        );
    }
    for (url, why) in ALLOWED_EMBEDDED_V6 {
        check_probe_url_fast(url)
            .unwrap_or_else(|e| panic!("v6: probe guard must allow {why} ({url}), got '{e}'"));
    }
}

#[tokio::test]
async fn v6_data_plane_gate_blocks_nat64_families() {
    for (url, why) in BLOCKED_EMBEDDED_V6 {
        assert!(
            check_data_plane_url(url).await.is_err(),
            "v6: data-plane guard must refuse {why} ({url}), but it was accepted"
        );
    }
    for (url, why) in ALLOWED_EMBEDDED_V6 {
        check_data_plane_url(url)
            .await
            .unwrap_or_else(|e| panic!("v6: data-plane guard must allow {why} ({url}), got {e:?}"));
    }
}

// ---------------------------------------------------------------------------
// 3. parser parity between the hand-rolled fast path and the WHATWG parser
// ---------------------------------------------------------------------------

/// Each input resolves to a private/metadata target under the WHATWG parser
/// (config gate) but is currently waved through by `check_proxy_url_fast`'s
/// hand-rolled host splitter. v6 requires BOTH gates to make the same
/// (blocking) decision — the fast path must use the same parser semantics.
#[test]
fn v6_fast_path_and_config_gate_agree_on_parser_tricks() {
    for (url, why) in [
        (
            "http://10.0.0.5:8080#@127.0.0.1",
            "fragment trick (WHATWG host = 10.0.0.5)",
        ),
        ("http://0xa000005:3128", "hex IPv4 encoding of 10.0.0.5"),
        ("http://2852039166:80", "decimal IPv4 encoding of 10.0.0.5"),
        (
            "http:// 10.0.0.5:8080",
            "leading space inside the authority",
        ),
    ] {
        let proxy_verdict = check_proxy_url_fast(url);
        let config_verdict = validate_egress_entry(url);
        assert_eq!(
            proxy_verdict.is_err(),
            config_verdict.is_err(),
            "v6: parser disagreement for {why} ({url}): check_proxy_url_fast = {proxy_verdict:?}, validate_egress_entry = {config_verdict:?}"
        );
        assert!(
            config_verdict.is_err(),
            "v6: the WHATWG config gate must block {why} ({url}), got {config_verdict:?}"
        );
        assert!(
            proxy_verdict.is_err(),
            "v6: check_proxy_url_fast must block {why} ({url}) exactly like the config gate, got {proxy_verdict:?}"
        );
    }
}
