//! FROZEN RED-PHASE ACCEPTANCE — Wave-2 P1 egress IPv6 embedding bypass (M1)
//! and the socks5 egress-entry rejection (M2 cross-layer consistency).
//!
//! Contract (frozen by Lead, v3/v4), IPv6 literals only (deterministic, no DNS):
//!
//! - 6to4 `2002::/16`      — the embedded IPv4 (bits 16..48) MUST be judged by
//!   the shared IPv4 policy;
//! - Teredo `2001::/32`    — BOTH the embedded server IPv4 (bits 32..64) and the
//!   obfuscated client IPv4 (last 32 bits XOR `0xffffffff`) MUST be judged;
//! - ISATAP `::5efe:*` / `::200:5efe:*` interface IDs — the embedded IPv4 MUST
//!   be judged even under an otherwise-public prefix;
//! - global unicast addresses (`2001:db8::1` and real public resolvers) MUST
//!   stay allowed — this is the over-tightening guard.
//!
//! Every case is asserted through all three public guards (proxy / probe /
//! data-plane) because each one owns a separate policy entry point that has
//! historically drifted apart.
//!
//! Fixtures deliberately embed `10.0.0.1` (private) and `169.254.169.254`
//! (cloud metadata) instead of loopback: loopback is legitimately unblocked by
//! `PONYLLM_ALLOW_LOOPBACK_PROBE=1`, so asserting on it would be
//! environment-dependent (flaky) rather than a real contract check.

use ponyllm_server::egress::{
    check_data_plane_url, check_egress_pool_entry, check_probe_url_fast, check_proxy_url_fast,
};

/// Blocked IPv6 literals with an embedded private / metadata IPv4.
/// `(url, why)`
const BLOCKED_V6_URLS: &[(&str, &str)] = &[
    // 6to4 2002::/16 — embedded 10.0.0.1
    (
        "http://[2002:0a00:0001::1]:8899",
        "6to4 embedded private 10.0.0.1",
    ),
    // 6to4 2002::/16 — embedded 169.254.169.254 (cloud metadata)
    (
        "http://[2002:a9fe:a9fe::1]:8899",
        "6to4 embedded metadata 169.254.169.254",
    ),
    // Teredo 2001::/32 — embedded *server* IPv4 10.0.0.1
    (
        "http://[2001:0:0a00:0001::1]:8899",
        "Teredo embedded server IPv4 10.0.0.1",
    ),
    // Teredo 2001::/32 — obfuscated *client* IPv4 = 10.0.0.1 ^ 0xffffffff
    (
        "http://[2001:0:0:0:0:0:f5ff:fffe]:8899",
        "Teredo obfuscated client IPv4 10.0.0.1",
    ),
    // Teredo 2001::/32 — obfuscated client IPv4 = 169.254.169.254 ^ 0xffffffff
    (
        "http://[2001:0:0:0:0:0:5601:5601]:8899",
        "Teredo obfuscated client IPv4 169.254.169.254",
    ),
    // ISATAP interface id ::5efe:<v4> under a public prefix
    (
        "http://[2001:db8:1::5efe:0a00:0001]:8899",
        "ISATAP ::5efe: embedded private 10.0.0.1",
    ),
    // ISATAP `::200:5efe:` variant under a public prefix
    (
        "http://[2001:db8:2::200:5efe:a9fe:a9fe]:8899",
        "ISATAP ::200:5efe: embedded metadata 169.254.169.254",
    ),
];

/// Legal public IPv6 literals that must keep working (no over-tightening).
const ALLOWED_V6_URLS: &[(&str, &str)] = &[
    (
        "http://[2001:db8::1]:8899",
        "documentation prefix 2001:db8::/32",
    ),
    (
        "https://[2606:4700:4700::1111]:443",
        "Cloudflare public resolver",
    ),
    (
        "https://[2001:4860:4860::8888]:443",
        "Google public resolver",
    ),
];

// ---------------------------------------------------------------------------
// M1 — embedded IPv4 in 6to4 / Teredo / ISATAP is judged (blocked)
// ---------------------------------------------------------------------------

#[test]
fn m1_proxy_guard_blocks_embedded_ipv4_families() {
    for (url, why) in BLOCKED_V6_URLS {
        let verdict = check_proxy_url_fast(url);
        let err = verdict.err().unwrap_or_else(|| {
            panic!("M1: proxy guard must refuse {why} ({url}), but it was accepted")
        });
        assert!(
            err.contains("blocked"),
            "M1: refusal for {why} must explain the blocked address, got '{err}'"
        );
    }
}

#[test]
fn m1_probe_guard_blocks_embedded_ipv4_families() {
    for (url, why) in BLOCKED_V6_URLS {
        let err = check_probe_url_fast(url).err().unwrap_or_else(|| {
            panic!("M1: probe guard must refuse {why} ({url}), but it was accepted")
        });
        assert!(
            err.contains("blocked"),
            "M1: probe refusal for {why} must explain the blocked address, got '{err}'"
        );
    }
}

#[tokio::test]
async fn m1_data_plane_guard_blocks_embedded_ipv4_families() {
    for (url, why) in BLOCKED_V6_URLS {
        let verdict = check_data_plane_url(url).await;
        assert!(
            verdict.is_err(),
            "M1: data-plane guard must refuse {why} ({url}), but it was accepted"
        );
    }
}

// ---------------------------------------------------------------------------
// M1 — global unicast IPv6 stays legal on every guard (over-tightening guard)
// ---------------------------------------------------------------------------

#[test]
fn m1_public_ipv6_literals_stay_allowed_on_proxy_and_probe() {
    for (url, why) in ALLOWED_V6_URLS {
        check_proxy_url_fast(url)
            .unwrap_or_else(|e| panic!("M1: proxy guard must allow {why} ({url}), got '{e}'"));
        check_probe_url_fast(url)
            .unwrap_or_else(|e| panic!("M1: probe guard must allow {why} ({url}), got '{e}'"));
    }
}

#[tokio::test]
async fn m1_public_ipv6_literals_stay_allowed_on_data_plane() {
    for (url, why) in ALLOWED_V6_URLS {
        check_data_plane_url(url)
            .await
            .unwrap_or_else(|e| panic!("M1: data-plane guard must allow {why} ({url}), got {e:?}"));
    }
}

// ---------------------------------------------------------------------------
// M2 cross-layer check — socks5/socks5h egress entries are refused through the
// server's delegate as well as through the config validator.
// ---------------------------------------------------------------------------

#[test]
fn m2_egress_pool_entry_rejects_socks_proxies() {
    for entry in [
        "socks5://192.0.2.10:1080",
        "socks5h://192.0.2.10:1080",
        "socks5://[2606:4700:4700::1111]:1080",
    ] {
        let err = check_egress_pool_entry(entry).err().unwrap_or_else(|| {
            panic!("M2: egress-pool entry '{entry}' must be refused (workspace reqwest has no socks feature)")
        });
        assert!(
            err.to_ascii_lowercase().contains("socks"),
            "M2: refusal for '{entry}' must name the socks scheme, got '{err}'"
        );
    }

    // http(s) entries keep working (over-tightening guard).
    for entry in [
        "direct",
        "none",
        "http://127.0.0.1:8899",
        "http://192.0.2.10:3128",
    ] {
        check_egress_pool_entry(entry).unwrap_or_else(|e| {
            panic!("M2: egress-pool entry '{entry}' must stay legal, got '{e}'")
        });
    }
}
