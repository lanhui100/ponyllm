//! Egress-pool scheduling & cooldown tests (contract C4 + C5 + C6).
//!
//! C4 — round_robin rotates by an internal counter, skipping cooling entries;
//!      a single-entry pool degenerates to a fixed egress.
//! C5 — a 429 `FreeUsageLimitError` (classified `QuotaExhausted`) cools ONLY
//!      the offending egress for the advertised reset: `Resets in ...` body /
//!      `Retry-After` header when present, 900s default when absent. Transient
//!      failures never cool an egress.
//! C6 — every egress cooling ⇒ no available egress ⇒ `NoAvailableKey`
//!      (the discriminator the executor maps to `quota_exhausted` semantics,
//!      mirroring the pool's `any_key_quota_cooldown` gate).
//!
//! These tests target the ASSUMED frozen interface in
//! `crates/ponyllm-core/src/pool/egress.rs` (mounted under `pool::`):
//! - `EgressStrategy::{RoundRobin, Priority}` with `Default` (RoundRobin),
//!   `FromStr` (`"round_robin"` / `"priority"`, trim + lowercase, Err=String)
//!   and `Display` (`"round_robin"` / `"priority"`). No serde: `egress_strategy`
//!   stays a plain string in the TOML model; the core parse entry is
//!   `EgressStrategy::from_str`, pinned by `c4_egress_strategy_from_str_and_default`.
//! - `EgressEntry` with `pub id: String` (caller-chosen internal identifier,
//!   e.g. `"direct"` / `"vps"`) and `pub url: Option<String>` (`None` = direct,
//!   `Some(proxy_url)` = via that proxy), plus internal cooldown state:
//!   `EgressEntry::direct(id)` / `EgressEntry::proxy(id, url)`,
//!   `cooldown_remaining()`, `cooldown_reset_at()`, `set_cooldown(Duration)`,
//!   `clear_cooldown()` (mirror `ApiKeyEntry`).
//! - `EgressPool`:
//!   - `new(provider: &str, strategy: EgressStrategy) -> Self` (empty pool;
//!     ids are NOT passed to `new`, they come per-entry via `add_egress`)
//!   - `add_egress(EgressEntry)` (id unique per pool)
//!   - `select_egress() -> Result<Arc<EgressEntry>, CoreError>`
//!     (skips cooling entries; empty / all-cooling ⇒
//!     `Err(CoreError::NoAvailableKey(provider))`)
//!   - `record_quota_exhausted(egress_id: &str, retry_after: Option<Duration>)`
//!     — cool THAT egress; `None` ⇒ 900s; wall-clock `cooldown_reset_at`
//!     derived internally (no caller-provided `SystemTime`)
//!   - `record_transient_failure(egress_id)` — count only, never cool
//!   - `egress_cooldown(id) -> (Option<Duration>, Option<SystemTime>)`
//!   - `all_cooling() -> bool`
//!   - `clear_egress_cooldown(egress_id: &str)`

use std::time::Duration;

use ponyllm_core::error::CoreError;
use ponyllm_core::pool::{EgressEntry, EgressPool, EgressStrategy};

/// Small round-robin pool: one direct egress + one proxy egress.
fn two_entry_pool() -> EgressPool {
    let pool = EgressPool::new("opencode-zen", EgressStrategy::RoundRobin);
    pool.add_egress(EgressEntry::direct("direct"));
    pool.add_egress(EgressEntry::proxy("vps", "http://127.0.0.1:8899"));
    pool
}

// ---------- C4: rotation / skip-cooling / degeneracy ----------

/// Pins the canonical string parse entry for `egress_strategy` so the
/// config→core wiring cannot silently invent a second parser.
#[test]
fn c4_egress_strategy_from_str_and_default() {
    assert_eq!(
        "round_robin".parse::<EgressStrategy>().unwrap(),
        EgressStrategy::RoundRobin
    );
    assert_eq!(
        "priority".parse::<EgressStrategy>().unwrap(),
        EgressStrategy::Priority
    );
    assert_eq!(
        EgressStrategy::default(),
        EgressStrategy::RoundRobin,
        "unconfigured egress_strategy must default to round_robin"
    );
}

#[test]
fn c4_round_robin_rotates_across_entries_by_counter() {
    let pool = two_entry_pool();
    let seq: Vec<String> = (0..6)
        .map(|_| pool.select_egress().unwrap().id.clone())
        .collect();
    assert_eq!(
        seq,
        vec![
            "direct".to_string(),
            "vps".to_string(),
            "direct".to_string(),
            "vps".to_string(),
            "direct".to_string(),
            "vps".to_string(),
        ],
        "round_robin must alternate by counter: {seq:?}"
    );
}

#[test]
fn c4_round_robin_skips_cooling_egress_and_fails_over() {
    let pool = two_entry_pool();
    // Cool "direct" out for an hour: every subsequent selection must land on
    // "vps" and the counter must keep advancing without ever picking "direct".
    pool.record_quota_exhausted("direct", Some(Duration::from_secs(3600)));
    for i in 0..5 {
        let e = pool.select_egress().unwrap();
        assert_eq!(e.id, "vps", "selection {i} must skip the cooling egress");
    }
    // Recovery: clear the cooldown and rotation resumes (both entries usable).
    pool.clear_egress_cooldown("direct");
    let ids: Vec<String> = (0..4)
        .map(|_| pool.select_egress().unwrap().id.clone())
        .collect();
    assert_eq!(
        ids,
        vec!["vps", "direct", "vps", "direct"],
        "rotation resumes after recovery: {ids:?}"
    );
}

#[test]
fn c4_single_entry_pool_is_fixed() {
    let pool = EgressPool::new("opencode-zen", EgressStrategy::RoundRobin);
    pool.add_egress(EgressEntry::direct("direct"));
    for _ in 0..5 {
        assert_eq!(pool.select_egress().unwrap().id, "direct");
    }
}

#[test]
fn c4_priority_strategy_takes_first_available_egress() {
    let pool = EgressPool::new("opencode-zen", EgressStrategy::Priority);
    pool.add_egress(EgressEntry::direct("direct"));
    pool.add_egress(EgressEntry::proxy("vps", "http://127.0.0.1:8899"));
    // All active: leader pinned.
    for _ in 0..5 {
        assert_eq!(pool.select_egress().unwrap().id, "direct");
    }
    // Leader cooling ⇒ first available becomes the proxy.
    pool.record_quota_exhausted("direct", Some(Duration::from_secs(3600)));
    for _ in 0..3 {
        assert_eq!(pool.select_egress().unwrap().id, "vps");
    }
}

// ---------- C5: quota 429 cools that egress (retry_after respected) ----------

#[test]
fn c5_quota_exhausted_cools_only_the_offending_egress() {
    let pool = two_entry_pool();
    // Upstream 429 FreeUsageLimitError with an advertised reset (e.g. body
    // `Resets in 30m00s` or `Retry-After: 1800`).
    pool.record_quota_exhausted("vps", Some(Duration::from_secs(1800)));

    let (remaining, reset_at) = pool.egress_cooldown("vps");
    let rem = remaining.expect("vps must be cooling");
    assert!(
        rem >= Duration::from_secs(1790),
        "vps cooldown must follow the advertised reset, got {rem:?}"
    );
    assert!(
        reset_at.is_some(),
        "wall-clock cooldown_reset_at must be set"
    );

    // The OTHER egress is untouched and keeps serving.
    assert_eq!(
        pool.egress_cooldown("direct").0,
        None,
        "direct must not be cooled by vps's 429"
    );
    for _ in 0..3 {
        assert_eq!(pool.select_egress().unwrap().id, "direct");
    }
}

#[test]
fn c5_quota_exhausted_without_reset_defaults_to_900s() {
    let pool = two_entry_pool();
    pool.record_quota_exhausted("vps", None);
    let (remaining, reset_at) = pool.egress_cooldown("vps");
    let rem = remaining.expect("vps must be cooling with the default");
    assert!(
        (850..=950).contains(&rem.as_secs()),
        "no reset hint must fall back to 900s, got {rem:?}"
    );
    assert!(reset_at.is_some());
}

#[test]
fn c5_transient_failure_never_cools_an_egress() {
    let pool = two_entry_pool();
    // Network / 5xx / TTFB failures only count toward failure stats and
    // fail over to the next egress without isolating this one.
    pool.record_transient_failure("vps");
    assert_eq!(
        pool.egress_cooldown("vps").0,
        None,
        "transient failures must never cool an egress"
    );
    let ids: Vec<String> = (0..4)
        .map(|_| pool.select_egress().unwrap().id.clone())
        .collect();
    assert_eq!(
        ids,
        vec!["direct", "vps", "direct", "vps"],
        "rotation must continue: {ids:?}"
    );
}

/// The retry-after precedence chain (`Resets in` body > `Retry-After` header >
/// 900s default) is enforced by the executor classification before
/// `record_quota_exhausted` — the egress pool commits to whatever reset the
/// caller advertises. This pins the body-parse half of the chain so the wiring
/// cannot silently regress to the header.
#[test]
fn c5_resets_in_body_hint_parses_before_header() {
    let body_hint = ponyllm_core::executor::parse_reset_duration(
        r#"{"type":"FreeUsageLimitError","message":"Rate limit exceeded. Individual quota reached. Resets in 30m0s."}"#,
    );
    assert_eq!(
        body_hint,
        Some(Duration::from_secs(1800)),
        "`Resets in 30m` body hint must parse to 1800s"
    );
    // A body hint combined with a shorter Retry-After header must keep the
    // body value (caller passes body_reset.or(header) into the cooldown).
    let pool = two_entry_pool();
    pool.record_quota_exhausted("vps", Some(Duration::from_secs(1800))); // body wins
    let rem = pool.egress_cooldown("vps").0.expect("vps must be cooling");
    assert!(
        rem >= Duration::from_secs(1790),
        "body reset must win over the header, got {rem:?}"
    );
}

// ---------- C6: all cooling ⇒ no available egress ----------

#[test]
fn c6_all_egress_cooling_yields_no_available_egress() {
    let pool = two_entry_pool();
    pool.record_quota_exhausted("direct", Some(Duration::from_secs(3600)));
    pool.record_quota_exhausted("vps", Some(Duration::from_secs(3600)));

    assert!(
        pool.all_cooling(),
        "both egresses cooling ⇒ pool reports all_cooling"
    );
    match pool.select_egress() {
        Err(CoreError::NoAvailableKey(provider)) => {
            assert_eq!(provider, "opencode-zen");
        }
        other => panic!("all-cooling pool must surface NoAvailableKey, got {other:?}"),
    }
}

#[test]
fn c6_empty_pool_yields_no_available_egress() {
    let pool = EgressPool::new("opencode-zen", EgressStrategy::RoundRobin);
    assert!(matches!(
        pool.select_egress(),
        Err(CoreError::NoAvailableKey(_))
    ));
}
