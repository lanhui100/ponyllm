//! Egress-pool config surface tests (contract C1 + C3).
//!
//! C1 — `[providers.<name>]` gains `egress_pool` (list of egress entries) and
//!     `egress_strategy` (optional, defaults to `round_robin`):
//!     - TOML parse + serialize round-trip keeps the pool list order and the
//!       strategy verbatim;
//!     - a legacy provider WITHOUT the new fields keeps `proxy` untouched and
//!       resolves to an empty pool (zero migration / proxy semantics).
//! C3 — per-entry validation: `direct` / `none` / empty are legal; http(s)/
//!     socks5 proxies to public or loopback targets are legal; invalid schemes,
//!     missing hosts, and private / link-local / metadata targets are refused
//!     (same SSRF posture as the existing proxy guard).
//!
//! These tests target the ASSUMED frozen interface:
//! - `ProviderSection::egress_pool: Vec<String>` (empty = legacy proxy semantics)
//! - `ProviderSection::egress_strategy: String` (default `"round_robin"`)
//! - `ponyllm_config::validate_egress_entry(raw: &str) -> Result<(), String>`
//!   (may be implemented in ponyllm-config or re-exported from the egress
//!   guard — the tests only depend on the name and the policy below).

use ponyllm_config::validate_egress_entry;
use ponyllm_config::{ConfigFile, ProviderSection};

/// A provider with a full pool: one direct egress + two proxies.
const POOLED_TOML: &str = r#"
[gateway]
bind = "127.0.0.1:8080"
api_key = "sk-test"

[providers.opencode-zen]
base_url = "https://opencode.ai/zen"
default_model = "zen-free"
strategy = "round_robin"
egress_pool = ["direct", "http://127.0.0.1:8899", "socks5://egress-1.example.com:1080"]
egress_strategy = "round_robin"
keys = [ { id = "zen-1", api_key = "sk-zen-1", priority = 1, weight = 10 } ]
"#;

/// A provider with a pool but no explicit `egress_strategy` (default check).
const POOLED_NO_STRATEGY_TOML: &str = r#"
[gateway]
bind = "127.0.0.1:8080"
api_key = "sk-test"

[providers.opencode-zen]
base_url = "https://opencode.ai/zen"
default_model = "zen-free"
strategy = "round_robin"
egress_pool = ["direct"]
keys = [ { id = "zen-1", api_key = "sk-zen-1", priority = 1, weight = 10 } ]
"#;

/// A legacy provider: only the single `proxy` field, no egress fields at all.
const LEGACY_PROXY_TOML: &str = r#"
[gateway]
bind = "127.0.0.1:8080"
api_key = "sk-test"

[providers.opencode-zen]
base_url = "https://opencode.ai/zen"
default_model = "zen-free"
strategy = "round_robin"
proxy = "http://127.0.0.1:8899"
keys = [ { id = "zen-1", api_key = "sk-zen-1", priority = 1, weight = 10 } ]
"#;

fn parse_provider(toml: &str) -> ProviderSection {
    let cfg: ConfigFile = toml::from_str(toml).expect("TOML must parse");
    cfg.providers
        .get("opencode-zen")
        .expect("provider opencode-zen must exist")
        .clone()
}

// ---------- C1: parse / default / round-trip ----------

#[test]
fn c1_egress_pool_toml_parses_list_in_order() {
    let p = parse_provider(POOLED_TOML);
    assert_eq!(
        p.egress_pool,
        vec![
            "direct".to_string(),
            "http://127.0.0.1:8899".to_string(),
            "socks5://egress-1.example.com:1080".to_string(),
        ],
        "pool entries must parse verbatim and keep their order"
    );
    assert_eq!(p.egress_strategy, "round_robin");
}

#[test]
fn c1_egress_strategy_defaults_to_round_robin() {
    let p = parse_provider(POOLED_NO_STRATEGY_TOML);
    assert!(!p.egress_pool.is_empty());
    assert_eq!(
        p.egress_strategy, "round_robin",
        "missing egress_strategy must default to round_robin"
    );
}

#[test]
fn c1_egress_pool_serialize_round_trips() {
    let original = parse_provider(POOLED_TOML);
    let serialized = toml::to_string(&original).expect("serialize must not fail");
    // Round-trip at provider-section granularity: the bare section serializes
    // to a top-level TOML table (no `[providers.<name>]` wrapper — that key
    // is ConfigFile's business), so it must reparse as `ProviderSection`.
    let round: ProviderSection =
        toml::from_str(&serialized).expect("serialized TOML must reparse as a provider");
    assert_eq!(
        round.egress_pool, original.egress_pool,
        "round trip must preserve the pool list and order"
    );
    assert_eq!(round.egress_strategy, original.egress_strategy);
    assert_eq!(round.base_url, original.base_url);
    assert_eq!(round.proxy, original.proxy);
}

#[test]
fn c1_empty_pool_keeps_legacy_proxy_semantics() {
    let p = parse_provider(LEGACY_PROXY_TOML);
    assert!(
        p.egress_pool.is_empty(),
        "legacy provider must parse with an empty egress pool (zero migration)"
    );
    assert_eq!(
        p.egress_strategy, "round_robin",
        "legacy provider still defaults to round_robin"
    );
    assert_eq!(
        p.proxy.as_deref(),
        Some("http://127.0.0.1:8899"),
        "the single-proxy field must stay untouched when no pool is configured"
    );

    // Round trip keeps the proxy and stays pool-less (same section-level
    // granularity as c1_egress_pool_serialize_round_trips).
    let serialized = toml::to_string(&p).expect("serialize must not fail");
    let round: ProviderSection = toml::from_str(&serialized).expect("must reparse as a provider");
    assert!(round.egress_pool.is_empty());
    assert_eq!(round.proxy.as_deref(), Some("http://127.0.0.1:8899"));
}

// ---------- C3: entry validation ----------

#[test]
fn c3_validate_egress_accepts_direct_none_and_empty() {
    for entry in ["direct", "none", "  ", ""] {
        assert!(
            validate_egress_entry(entry).is_ok(),
            "entry '{entry:?}' must be legal (direct/none/empty = gateway node egress)"
        );
    }
}

#[test]
fn c3_validate_egress_accepts_public_and_loopback_proxies() {
    for entry in [
        "http://127.0.0.1:8899",           // local pproxy — documented shape
        "http://localhost:7890",           // local pproxy hostname form
        "http://1.2.3.4:8899",             // public literal IP
        "http://egress-1.example.com:8899",// public hostname
        "https://egress-2.example.com:443",
        "socks5://192.0.2.10:1080",        // documentation-test range, public
    ] {
        assert!(
            validate_egress_entry(entry).is_ok(),
            "entry '{entry}' must be accepted as a proxy egress"
        );
    }
}

#[test]
fn c3_validate_egress_rejects_bad_scheme_and_missing_host() {
    for entry in [
        "ftp://example.com:21",            // unsupported scheme
        "gopher://example.com/",           // unsupported scheme
        "file:///etc/passwd",              // unsupported scheme
        "no-scheme-here",                  // not scheme://host:port
        "http://",                         // no host
    ] {
        assert!(
            validate_egress_entry(entry).is_err(),
            "entry '{entry}' must be refused (scheme/host policy)"
        );
    }
}

#[test]
fn c3_validate_egress_rejects_private_linklocal_and_metadata_targets() {
    for entry in [
        "http://10.0.0.5:8080",                        // private 10/8
        "http://172.16.9.9:3128",                      // private 172.16/12
        "http://192.168.1.1:3128",                     // private 192.168/16
        "http://169.254.169.254:80",                   // link-local / cloud metadata
        "http://metadata.google.internal:80",          // metadata hostname
        "http://x.svc.cluster.local:8080",             // k8s in-cluster name
        "http://[::ffff:10.0.0.1]:8080",               // IPv4-mapped bypass
        "http://[fe80::1]:8080",                       // IPv6 link-local
    ] {
        assert!(
            validate_egress_entry(entry).is_err(),
            "entry '{entry}' must be refused (SSRF / private / metadata policy)"
        );
    }
}
