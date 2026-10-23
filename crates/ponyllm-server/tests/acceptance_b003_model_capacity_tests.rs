//! B003 acceptance tests — `/v1/models` model-capacity fields (context window /
//! max output) and the same-named cross-provider capacity-group max rule.
//!
//! Contract: `.dev-team/contracts/b003-model-capacity.md` (FROZEN).
//! Join keys: `CAP-01..CAP-15` (§9.1) and `J-01..J-06` (§9.2). Test name =
//! `cap_<nn>_<slug>` / `j_<nn>_<slug>` — one test per frozen join key.
//!
//! Design notes (L2 component/contract tier):
//! - Pure `AppState` contract tests cover aggregation/order/determinism.
//! - `J-*` tests bind a REAL axum listener on an ephemeral port (`create_app` +
//!   `axum::serve`, the production router) and assert the JSON wire contract;
//!   they never test only the in-memory method.
//! - CAP-12 is the contract's explicitly "static pattern" invariant (one
//!   `config.read()` snapshot per call): it scans `src/state.rs` at compile time
//!   via `include_str!` (the contract labels it "静态模式 + code review").

use std::sync::Arc;

use ponyllm_core::pool::UpstreamProtocol;
use ponyllm_server::config::{ModelSpec, ProviderConfig};
use ponyllm_server::routes::models::ParsedRequestModel;
use ponyllm_server::state::{ListedModel, RoutedTarget};
use ponyllm_server::{create_app, AppState, GatewayConfig};
use serde_json::Value;

// ─────────────────────────────────────────────────────────────────────────────
// Frozen numeric expectations (binary units, contract §4.3)
// ─────────────────────────────────────────────────────────────────────────────
const CTX_1M: u64 = 1_048_576;
const OUT_32K: u64 = 32_768;
const CTX_2M: u64 = 2_097_152;
const OUT_64K: u64 = 65_536;
const CTX_200K: u64 = 204_800;
const CTX_128K: u64 = 131_072;
const OUT_16K: u64 = 16_384;
const OUT_8K: u64 = 8_192;

// ─────────────────────────────────────────────────────────────────────────────
// Fixtures / helpers
// ─────────────────────────────────────────────────────────────────────────────

fn gateway_config(providers: Vec<(&str, ProviderConfig)>) -> GatewayConfig {
    let mut config = GatewayConfig::default();
    // Open auth keeps the acceptance probe focused on the model surface.
    config.auth_mode = ponyllm_config::AuthMode::Open;
    config.api_key = String::new();
    config.web_enabled = false;
    for (name, provider) in providers {
        config.providers.insert(name.to_string(), provider);
    }
    config
}

fn spec(name: &str, context_window: &str, max_output: &str) -> ModelSpec {
    ModelSpec {
        name: name.to_string(),
        context_window: context_window.to_string(),
        max_output: max_output.to_string(),
        ..ModelSpec::default()
    }
}

fn spec_with(
    name: &str,
    context_window: &str,
    max_output: &str,
    display_name: Option<&str>,
    protocol: Option<UpstreamProtocol>,
) -> ModelSpec {
    ModelSpec {
        name: name.to_string(),
        context_window: context_window.to_string(),
        max_output: max_output.to_string(),
        display_name: display_name.map(str::to_string),
        protocol,
        ..ModelSpec::default()
    }
}

fn provider(default_model: &str, models: &[&str], model_specs: Vec<ModelSpec>) -> ProviderConfig {
    ProviderConfig {
        base_url: "https://example.invalid/v1".to_string(),
        default_model: default_model.to_string(),
        strategy: "round_robin".to_string(),
        models: models.iter().map(|m| m.to_string()).collect(),
        model_specs,
        ..ProviderConfig::default()
    }
}

/// Contract §4.6 fixture A — cross-provider same-named max + a provider-local model.
fn fixture_a() -> GatewayConfig {
    gateway_config(vec![
        (
            "alpha",
            provider(
                "shared",
                &["shared-lite"],
                vec![
                    spec("shared", "1M", "64K"),
                    spec("shared-lite", "128K", "16K"),
                ],
            ),
        ),
        (
            "beta",
            provider("shared", &[], vec![spec("shared", "2M", "32K")]),
        ),
    ])
}

/// Fixture A plus a multi-segment model provider (used by the HTTP `J-*` tests).
fn fixture_http() -> GatewayConfig {
    let mut config = fixture_a();
    config.providers.insert(
        "zen".to_string(),
        provider(
            "convaiinnovations/laya",
            &[],
            vec![spec("convaiinnovations/laya", "200K", "16K")],
        ),
    );
    config
}

/// Entry/order/dedup fixture: alias, gemini variants, multi-segment, duplicate
/// `default_model` in `models`, and a physical model literally named `auto`.
fn fixture_composite() -> GatewayConfig {
    gateway_config(vec![
        (
            "alpha",
            provider(
                "deepseek-flash",
                &["deepseek-flash", "shared-model"],
                vec![
                    spec("deepseek-flash", "128K", "16K"),
                    spec("shared-model", "64K", "8K"),
                ],
            ),
        ),
        (
            "g",
            provider(
                "",
                &["gemini-3.8-flash-high", "gemini-3.8-flash-low"],
                vec![
                    spec_with(
                        "gemini-3.8-flash-high",
                        "2M",
                        "64K",
                        Some("G3.8F High"),
                        Some(UpstreamProtocol::Chat),
                    ),
                    spec_with(
                        "gemini-3.8-flash-low",
                        "1M",
                        "32K",
                        Some("G3.8F Low"),
                        Some(UpstreamProtocol::Chat),
                    ),
                ],
            ),
        ),
        (
            "misc",
            provider("auto", &[], vec![spec("auto", "32K", "4K")]),
        ),
        (
            "zen",
            provider(
                "convaiinnovations/laya",
                &[],
                vec![spec("convaiinnovations/laya", "200K", "16K")],
            ),
        ),
    ])
}

fn capacity_of(state: &AppState, id: &str) -> (u64, u64) {
    let models = state.list_all_models_with_capacity();
    let found = models.iter().find(|m| m.id == id).unwrap_or_else(|| {
        panic!(
            "missing listed model '{id}'; actual ids = {:?}",
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>()
        )
    });
    (found.context_window, found.max_output)
}

fn ids_of(state: &AppState) -> Vec<String> {
    state
        .list_all_models_with_capacity()
        .into_iter()
        .map(|m| m.id)
        .collect()
}

fn legacy_ids(state: &AppState) -> Vec<String> {
    state
        .list_all_models()
        .into_iter()
        .map(|(id, _, _, _)| id)
        .collect()
}

/// Contract §5.1 — the frozen 13 keys, in order (preserve_order is workspace-wide).
fn expected_model_keys() -> Vec<String> {
    [
        "id",
        "object",
        "type",
        "created",
        "created_at",
        "owned_by",
        "display_name",
        "protocol",
        "permission",
        "root",
        "parent",
        "context_window",
        "max_output",
    ]
    .iter()
    .map(|k| k.to_string())
    .collect()
}

fn keys_of(model: &Value) -> Vec<String> {
    model
        .as_object()
        .expect("model entry must be a JSON object")
        .keys()
        .cloned()
        .collect()
}

async fn spawn_gateway(state: Arc<AppState>) -> std::net::SocketAddr {
    let app = create_app(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

async fn get_json(addr: std::net::SocketAddr, path: &str) -> (u16, Value) {
    let resp = reqwest::Client::new()
        .get(format!("http://{addr}{path}"))
        .send()
        .await
        .unwrap_or_else(|e| panic!("GET {path} must complete, got transport error: {e}"));
    let status = resp.status().as_u16();
    let body = resp
        .json::<Value>()
        .await
        .unwrap_or_else(|e| panic!("GET {path} must return JSON, got: {e}"));
    (status, body)
}

// ─────────────────────────────────────────────────────────────────────────────
// CAP — structure / capacity / order (§9.1)
// ─────────────────────────────────────────────────────────────────────────────

/// CAP-01 — `ListedModel` is a module-level `pub` struct with exactly the six
/// frozen fields / types (§2). Constructed field-by-field at compile time.
#[test]
fn cap_01_listed_model_struct_shape() {
    // Compile-time shape proof: field names and types must match §2 exactly.
    let model = ListedModel {
        id: "auto".to_string(),
        provider: "ponyllm".to_string(),
        display_name: Some("display".to_string()),
        protocol: "auto".to_string(),
        context_window: 1u64,
        max_output: 1u64,
    };
    let _: &String = &model.id;
    let _: &String = &model.provider;
    let _: &Option<String> = &model.display_name;
    let _: &String = &model.protocol;
    let _: u64 = model.context_window;
    let _: u64 = model.max_output;

    fn requires_eq<T: Eq>() {}
    requires_eq::<ListedModel>();

    // Derives required by §2: Debug + Clone + PartialEq.
    let cloned = model.clone();
    assert_eq!(model, cloned);
    assert!(format!("{model:?}").contains("ListedModel"));
}

/// CAP-02 — the virtual auto entry is first and carries the frozen four fields
/// (§3.2-1, INV-02).
#[test]
fn cap_02_auto_entry_is_first_with_frozen_fields() {
    let state = AppState::new(fixture_a());
    let models = state.list_all_models_with_capacity();

    assert!(
        !models.is_empty(),
        "listing must never be empty (auto is virtual)"
    );
    let auto = &models[0];
    assert_eq!(auto.id, "auto");
    assert_eq!(auto.provider, "ponyllm");
    assert_eq!(auto.display_name.as_deref(), Some("Auto(智能·高可用主力)"));
    assert_eq!(auto.protocol, "auto");
    assert_eq!(
        models.iter().filter(|m| m.id == "auto").count(),
        1,
        "auto must appear exactly once"
    );
}

/// CAP-03 — cross-provider same-named capacity group takes the component-wise
/// max of every provider (§4.1, §4.6 fixture A).
#[test]
fn cap_03_cross_provider_same_name_takes_group_max() {
    let state = AppState::new(fixture_a());

    // Group `shared`: ctx max(1M, 2M) = 2M, out max(64K, 32K) = 64K.
    assert_eq!(capacity_of(&state, "auto"), (CTX_2M, OUT_64K));
    assert_eq!(capacity_of(&state, "alpha/shared"), (CTX_2M, OUT_64K));
    assert_eq!(capacity_of(&state, "beta/shared"), (CTX_2M, OUT_64K));
    // Provider-local (`shared-lite`) keeps its own group value.
    assert_eq!(
        capacity_of(&state, "alpha/shared-lite"),
        (CTX_128K, OUT_16K)
    );

    // Neither the first provider's local values nor the default spec may leak.
    assert_ne!(
        capacity_of(&state, "alpha/shared"),
        (CTX_1M, OUT_64K),
        "alpha/shared must be raised to beta's 2M context (group max)"
    );
}

/// CAP-04 — empty providers list and single-model boundary: no panic, positive
/// capacity and the frozen default (B01, INV-04, INV-05).
#[test]
fn cap_04_empty_and_single_model_boundaries() {
    // Empty config: exactly `[auto]`, default capacity, no panic.
    let empty = AppState::new(gateway_config(vec![]));
    let models = empty.list_all_models_with_capacity();
    assert_eq!(models.len(), 1, "empty providers must list only auto");
    assert_eq!(models[0].id, "auto");
    assert_eq!(
        (models[0].context_window, models[0].max_output),
        (CTX_1M, OUT_32K)
    );

    // Single provider / single model without any spec: still default 1M/32K
    // and strictly positive (§4.2-3 B08/B01).
    let single = AppState::new(gateway_config(vec![(
        "solo",
        provider("solo-model", &[], vec![]),
    )]));
    let models = single.list_all_models_with_capacity();
    assert_eq!(ids_of(&single), vec!["auto", "solo/solo-model"]);
    for model in &models {
        assert!(
            model.context_window > 0,
            "{:?} context_window must be > 0",
            model.id
        );
        assert!(
            model.max_output > 0,
            "{:?} max_output must be > 0",
            model.id
        );
    }
    assert_eq!(capacity_of(&single, "solo/solo-model"), (CTX_1M, OUT_32K));
}

/// CAP-05 — alias entries are covered by their canonical same-named group: never
/// their own alias spec and never `ModelSpec::default()` (§4.4, INV-06; user
/// acceptance case: A `deepseek-flash` 200K/16K + B `deepseek-v4.1-flash` 128K/8K).
#[test]
fn cap_05_alias_entry_uses_canonical_group_max() {
    let state = AppState::new(gateway_config(vec![
        (
            "alpha",
            provider(
                "deepseek-flash",
                &[],
                vec![spec("deepseek-flash", "200K", "16K")],
            ),
        ),
        (
            "beta",
            provider(
                "deepseek-v4.1-flash",
                &[],
                vec![spec("deepseek-v4.1-flash", "128K", "8K")],
            ),
        ),
    ]));

    assert_eq!(
        ids_of(&state),
        vec![
            "auto",
            "alpha/deepseek-flash",
            // alias derived from alpha's default_model
            "alpha/deepseek-v4.1-flash",
            "beta/deepseek-v4.1-flash",
        ],
        "both providers' entries plus the alias-derived entry must be listed"
    );

    // Group `deepseek-flash` = max(200K, 128K) / max(16K, 8K) = 200K/16K.
    for id in [
        "auto",
        "alpha/deepseek-flash",
        "alpha/deepseek-v4.1-flash",
        "beta/deepseek-v4.1-flash",
    ] {
        assert_eq!(
            capacity_of(&state, id),
            (CTX_200K, OUT_16K),
            "{id} must take the canonical `deepseek-flash` group max"
        );
    }

    // Explicit anti-regression: the alias entry must NOT fall back to the default
    // ModelSpec (1M/32K) nor surface its own alias spec (128K/8K).
    let alias_cap = capacity_of(&state, "alpha/deepseek-v4.1-flash");
    assert_ne!(
        alias_cap,
        (CTX_1M, OUT_32K),
        "alias entry fell back to ModelSpec::default()"
    );
    assert_ne!(
        alias_cap,
        (CTX_128K, OUT_8K),
        "alias entry used its own spec instead of the group max"
    );
}

/// CAP-06 — gemini-3.8-flash variants normalize to one id; capacity from the
/// canonical group, display/protocol from the variant spec (§3.2-5/6/7, fixture C).
#[test]
fn cap_06_gemini_variants_normalize_and_keep_variant_display() {
    let state = AppState::new(gateway_config(vec![(
        "g",
        provider(
            "",
            &["gemini-3.8-flash-high"],
            vec![spec_with(
                "gemini-3.8-flash-high",
                "2M",
                "64K",
                Some("G3.8F High"),
                Some(UpstreamProtocol::Chat),
            )],
        ),
    )]));

    assert_eq!(ids_of(&state), vec!["auto", "g/gemini-3.8-flash"]);
    let entry = state
        .list_all_models_with_capacity()
        .into_iter()
        .find(|m| m.id == "g/gemini-3.8-flash")
        .expect("normalized id must exist");
    assert_eq!((entry.context_window, entry.max_output), (CTX_2M, OUT_64K));
    assert_eq!(entry.display_name.as_deref(), Some("G3.8F High"));
    assert_eq!(entry.protocol, "chat");
    assert_eq!(capacity_of(&state, "auto"), (CTX_2M, OUT_64K));
}

/// CAP-07 — multi-segment model names strip exactly one `<provider>/` prefix
/// (B06): `p/convaiinnovations/laya` groups under `convaiinnovations/laya`,
/// together with the `laya-free` alias of another provider.
#[test]
fn cap_07_multi_segment_model_strips_provider_prefix_once() {
    let state = AppState::new(gateway_config(vec![
        (
            "p",
            provider(
                "convaiinnovations/laya",
                &[],
                vec![spec("convaiinnovations/laya", "200K", "16K")],
            ),
        ),
        (
            "q",
            provider("laya-free", &[], vec![spec("laya-free", "128K", "8K")]),
        ),
    ]));

    assert_eq!(
        ids_of(&state),
        vec![
            "auto",
            "p/convaiinnovations/laya",
            "p/laya",
            "p/laya-free",
            "q/laya-free",
        ]
    );
    // `laya-free` canonicalizes to `convaiinnovations/laya`; the group max is 200K/16K.
    for id in [
        "p/convaiinnovations/laya",
        "p/laya",
        "p/laya-free",
        "q/laya-free",
    ] {
        assert_eq!(
            capacity_of(&state, id),
            (CTX_200K, OUT_16K),
            "{id} must group under `convaiinnovations/laya`"
        );
    }
    assert_ne!(
        capacity_of(&state, "p/convaiinnovations/laya"),
        (CTX_128K, OUT_8K),
        "must not group by a secondary `/` split or by q's local spec"
    );
}

/// CAP-08 — a model with no spec contributes `ModelSpec::default()` (1M/32K) and
/// participates in the group max (B08, §4.1).
#[test]
fn cap_08_model_without_spec_contributes_default_capacity() {
    let state = AppState::new(gateway_config(vec![
        ("r", provider("nospec-model", &[], vec![])),
        (
            "s",
            provider(
                "nospec-model",
                &[],
                vec![spec("nospec-model", "128K", "16K")],
            ),
        ),
    ]));

    // max(default 1M, 128K) = 1M; max(default 32K, 16K) = 32K.
    for id in ["auto", "r/nospec-model", "s/nospec-model"] {
        assert_eq!(
            capacity_of(&state, id),
            (CTX_1M, OUT_32K),
            "{id} must include the default-spec contribution"
        );
    }
    assert_ne!(
        capacity_of(&state, "r/nospec-model"),
        (CTX_128K, OUT_16K),
        "the spec-less provider must still contribute the default capacity"
    );
}

/// CAP-09 — pathological `"0"` / `"0K"` capacities are clamped to 1 so every
/// entry stays strictly positive (B10, INV-04).
#[test]
fn cap_09_zero_capacity_is_clamped_to_one() {
    let state = AppState::new(gateway_config(vec![(
        "z",
        provider("zero-model", &[], vec![spec("zero-model", "0", "0K")]),
    )]));

    for id in ["auto", "z/zero-model"] {
        assert_eq!(capacity_of(&state, id), (1, 1), "{id} must clamp to (1, 1)");
    }
    for model in state.list_all_models_with_capacity() {
        assert!(
            model.context_window > 0,
            "{:?} context_window must be > 0",
            model.id
        );
        assert!(
            model.max_output > 0,
            "{:?} max_output must be > 0",
            model.id
        );
    }
}

/// CAP-10 — entry set/order/dedup is element-wise identical to legacy
/// `list_all_models()` (§3.2, INV-01): auto first, gemini normalized, no `[1m]`.
#[test]
fn cap_10_entry_order_and_dedup_match_legacy_listing() {
    let state = AppState::new(fixture_composite());
    let new_ids = ids_of(&state);
    let old_ids = legacy_ids(&state);

    assert_eq!(
        new_ids, old_ids,
        "capacity listing must preserve entry order"
    );
    assert_eq!(new_ids.first().map(String::as_str), Some("auto"));
    assert!(
        new_ids.contains(&"g/gemini-3.8-flash".to_string()),
        "gemini variant must be normalized to the base id: {new_ids:?}"
    );
    assert!(
        !new_ids.iter().any(|id| id.contains("gemini-3.8-flash-")),
        "variant suffix ids must not be listed: {new_ids:?}"
    );
    assert!(
        !new_ids.iter().any(|id| {
            let lower = id.to_ascii_lowercase();
            lower.contains("[1m]") || lower.contains("[1024k]")
        }),
        "listing must carry no [1m]/[1024k] tags: {new_ids:?}"
    );
    // Duplicate `default_model` in `models` collapses to one entry (B03).
    assert_eq!(
        new_ids
            .iter()
            .filter(|id| *id == "alpha/deepseek-flash")
            .count(),
        1
    );
    // INV-03: ids are unique.
    let unique: std::collections::HashSet<&String> = new_ids.iter().collect();
    assert_eq!(unique.len(), new_ids.len(), "listing ids must be unique");
}

/// CAP-11 — determinism: two consecutive calls are element-wise identical,
/// including capacity values and independent of HashMap iteration order (INV-07).
#[test]
fn cap_11_listing_is_deterministic() {
    let state = AppState::new(fixture_composite());
    let first = state.list_all_models_with_capacity();
    let second = state.list_all_models_with_capacity();
    assert_eq!(first, second, "listing must be deterministic across calls");
    assert!(!first.is_empty());
}

/// CAP-12 — one config snapshot per call: the implementation body contains
/// exactly one `self.config.read()` and is not left as a stub (§3.3, INV-10).
#[test]
fn cap_12_single_config_read_snapshot() {
    const STATE_SRC: &str = include_str!("../src/state.rs");
    fn method_body<'a>(src: &'a str, signature: &str) -> &'a str {
        let start = src
            .find(signature)
            .unwrap_or_else(|| panic!("missing method signature in state.rs: {signature}"));
        let after = &src[start..];
        let mut consumed = after
            .split_inclusive('\n')
            .next()
            .map(|line| line.len())
            .unwrap_or(0);
        for line in after.split_inclusive('\n').skip(1) {
            let trimmed = line.trim_start();
            let indent = line.len() - trimmed.len();
            let is_next_method = indent == 4
                && (trimmed.starts_with("fn ")
                    || trimmed.starts_with("pub fn ")
                    || trimmed.starts_with("pub async fn "));
            if is_next_method {
                return &after[..consumed];
            }
            consumed += line.len();
        }
        after
    }

    let body = method_body(
        STATE_SRC,
        "pub fn list_all_models_with_capacity(&self) -> Vec<ListedModel>",
    );

    assert!(
        !body.contains("unimplemented!"),
        "list_all_models_with_capacity must be implemented, not a stub:\n{body}"
    );
    let read_count = body.matches(".config.read()").count();
    assert_eq!(
        read_count, 1,
        "INV-10: exactly one config snapshot per call, found {read_count} in:\n{body}"
    );
}

/// CAP-13 — `list_all_models()` keeps its frozen tuple signature and maps
/// element-wise from the capacity listing (INV-08, INV-09).
#[test]
fn cap_13_legacy_listing_maps_elementwise_from_capacity_listing() {
    let state = AppState::new(fixture_composite());
    let legacy: Vec<(String, String, Option<String>, String)> = state.list_all_models();
    let capacity: Vec<ListedModel> = state.list_all_models_with_capacity();

    assert_eq!(legacy.len(), capacity.len());
    for (index, (legacy_entry, listed)) in legacy.iter().zip(capacity.iter()).enumerate() {
        assert_eq!(
            legacy_entry.0, listed.id,
            "entry #{index}: id mismatch between list_all_models and capacity listing"
        );
        assert_eq!(
            legacy_entry.1, listed.provider,
            "entry #{index} ({}): provider mismatch",
            listed.id
        );
        assert_eq!(
            legacy_entry.2, listed.display_name,
            "entry #{index} ({}): display_name mismatch",
            listed.id
        );
        assert_eq!(
            legacy_entry.3, listed.protocol,
            "entry #{index} ({}): protocol mismatch",
            listed.id
        );
    }
}

/// CAP-14 — capacity is discovery-only: routing resolution is identical when
/// the only config difference is the capacity strings (§1 Non-goals, INV-12).
///
/// Compared as a SORTED candidate set: `auto` candidate ordering legitimately
/// uses HashMap iteration order as its stable-sort tie-break (pre-existing
/// routing behavior, out of B003 scope), so order is not compared across two
/// separately built configs — membership and per-candidate routing attributes are.
#[test]
fn cap_14_capacity_does_not_drive_routing() {
    fn routing_config(context_window: &str, max_output: &str) -> GatewayConfig {
        gateway_config(vec![
            (
                "deepseek",
                provider(
                    "deepseek-flash",
                    &["deepseek-flash"],
                    vec![spec("deepseek-flash", context_window, max_output)],
                ),
            ),
            (
                "backup",
                provider(
                    "deepseek-flash",
                    &[],
                    vec![spec("deepseek-flash", context_window, max_output)],
                ),
            ),
        ])
    }

    let low = AppState::new(routing_config("128K", "16K"));
    let high = AppState::new(routing_config("2M", "64K"));

    let route_set =
        |targets: &[RoutedTarget]| -> Vec<(String, String, String, String, Option<u32>)> {
            let mut entries: Vec<_> = targets
                .iter()
                .map(|t| {
                    (
                        t.provider_name.clone(),
                        t.physical_model.clone(),
                        format!("{:?}", t.tier),
                        t.upstream_protocol.as_str().to_string(),
                        t.priority,
                    )
                })
                .collect();
            entries.sort();
            entries
        };

    for requested in ["deepseek-flash", "auto"] {
        let low_targets = low
            .resolve_routed_targets(&ParsedRequestModel::parse(requested), None)
            .unwrap_or_else(|e| {
                panic!("routing must resolve '{requested}' on low-capacity config: {e}")
            });
        let high_targets = high
            .resolve_routed_targets(&ParsedRequestModel::parse(requested), None)
            .unwrap_or_else(|e| {
                panic!("routing must resolve '{requested}' on high-capacity config: {e}")
            });
        assert!(
            !low_targets.is_empty(),
            "routing candidates must not be empty for {requested}"
        );
        assert_eq!(
            route_set(&low_targets),
            route_set(&high_targets),
            "capacity strings must not influence routing candidates for '{requested}'"
        );
    }

    // Fixture effectiveness: the two configs really do surface different capacities.
    assert_ne!(
        capacity_of(&low, "deepseek/deepseek-flash"),
        capacity_of(&high, "deepseek/deepseek-flash")
    );
}

/// CAP-15 — orphan `model_specs` entries (not in `default_model`/`models`) never
/// contribute capacity, even when the orphan name equals an alias entry's name
/// (§4.1, §4.4, B15).
#[test]
fn cap_15_orphan_specs_do_not_contribute_capacity() {
    let state = AppState::new(gateway_config(vec![(
        "p",
        provider(
            "deepseek-flash",
            &[],
            vec![
                spec("deepseek-flash", "128K", "16K"),
                // Orphan #1: name equals the alias entry name (`p/deepseek-v4.1-flash`).
                spec("deepseek-v4.1-flash", "2M", "64K"),
                // Orphan #2: unrelated name, still must not raise the global max.
                spec("orphan-only", "2M", "64K"),
            ],
        ),
    )]));

    assert_eq!(
        ids_of(&state),
        vec!["auto", "p/deepseek-flash", "p/deepseek-v4.1-flash"]
    );
    // Only the configured model contributes: group max stays 128K/16K.
    assert_eq!(capacity_of(&state, "p/deepseek-flash"), (CTX_128K, OUT_16K));
    assert_eq!(
        capacity_of(&state, "p/deepseek-v4.1-flash"),
        (CTX_128K, OUT_16K),
        "orphan spec named like the alias must not be read back for the alias entry"
    );
    assert_eq!(
        capacity_of(&state, "auto"),
        (CTX_128K, OUT_16K),
        "orphan specs must not raise the global max"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// J — JSON wire contract over a real axum listener (§9.2)
// ─────────────────────────────────────────────────────────────────────────────

/// J-01 — every list entry has exactly the frozen 13 keys, in the frozen order
/// (§5.1): the original 11 keys first, then `context_window` → `max_output`.
#[tokio::test]
async fn j_01_list_items_have_exactly_thirteen_keys() {
    let addr = spawn_gateway(Arc::new(AppState::new(fixture_http()))).await;
    let (status, body) = get_json(addr, "/v1/models").await;
    assert_eq!(status, 200, "GET /v1/models must be 200");
    let data = body["data"].as_array().expect("data must be an array");
    assert!(!data.is_empty(), "listing data must not be empty");
    for item in data {
        assert_eq!(
            keys_of(item),
            expected_model_keys(),
            "model entry keys/order must be the frozen 13 keys: {item}"
        );
    }
}

/// J-02 — `context_window` / `max_output` are JSON integers (not string/null/
/// float) and strictly positive (§5.1, INV-04).
#[tokio::test]
async fn j_02_capacity_fields_are_positive_json_integers() {
    let addr = spawn_gateway(Arc::new(AppState::new(fixture_http()))).await;
    let (status, body) = get_json(addr, "/v1/models").await;
    assert_eq!(status, 200);
    for item in body["data"].as_array().expect("data array") {
        for key in ["context_window", "max_output"] {
            let value = item
                .get(key)
                .unwrap_or_else(|| panic!("model {item} is missing '{key}'"));
            assert!(
                value.is_number(),
                "{key} must be a JSON number, got {value}"
            );
            assert!(
                value.is_u64(),
                "{key} must be an unsigned integer, got {value}"
            );
            assert!(!value.is_f64(), "{key} must not be a float, got {value}");
            let numeric = value.as_u64().expect("u64");
            assert!(numeric > 0, "{key} must be > 0, got {numeric}");
        }
    }
}

/// J-03 — the list envelope is byte-for-byte the frozen 5-key shape and `data`
/// order/length equals the capacity listing (auto first) (§5.2).
#[tokio::test]
async fn j_03_list_envelope_and_data_order_unchanged() {
    let state = Arc::new(AppState::new(fixture_http()));
    let addr = spawn_gateway(state.clone()).await;
    let (status, body) = get_json(addr, "/v1/models").await;
    assert_eq!(status, 200);

    let envelope = body.as_object().expect("envelope object");
    let envelope_keys: Vec<&str> = envelope.keys().map(String::as_str).collect();
    assert_eq!(
        envelope_keys,
        vec!["object", "data", "has_more", "first_id", "last_id"],
        "envelope must keep the frozen 5 keys"
    );
    assert_eq!(body["object"], "list");
    assert_eq!(body["has_more"], false);

    let expected = state.list_all_models_with_capacity();
    let data = body["data"].as_array().expect("data array");
    assert_eq!(
        data.len(),
        expected.len(),
        "data length must equal entry count"
    );
    let wire_ids: Vec<&str> = data.iter().map(|m| m["id"].as_str().expect("id")).collect();
    let expected_ids: Vec<&str> = expected.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(wire_ids, expected_ids, "data order must equal entry order");
    assert_eq!(
        wire_ids.first().copied(),
        Some("auto"),
        "auto must be first"
    );

    assert_eq!(
        body["first_id"], data[0]["id"],
        "first_id mirrors data[0].id"
    );
    assert_eq!(
        body["last_id"],
        data[data.len() - 1]["id"],
        "last_id mirrors data[-1].id"
    );
}

/// J-04 — the single-model detail endpoints (`/v1/models/{id}` and
/// `/v1/models/{provider}/{*model}`, plus the `/models` twins) return one 13-key
/// object whose capacity equals the same-id list entry (§5.2, §5.3).
#[tokio::test]
async fn j_04_single_model_detail_matches_list_capacities() {
    let state = Arc::new(AppState::new(fixture_http()));
    let addr = spawn_gateway(state.clone()).await;
    let list = state.list_all_models_with_capacity();
    let capacity_for = |id: &str| -> (u64, u64) {
        let entry = list
            .iter()
            .find(|m| m.id == id)
            .unwrap_or_else(|| panic!("missing list entry {id}"));
        (entry.context_window, entry.max_output)
    };

    for (path, expected_id) in [
        ("/v1/models/auto", "auto"),
        ("/v1/models/alpha/shared", "alpha/shared"),
        (
            "/v1/models/zen/convaiinnovations/laya",
            "zen/convaiinnovations/laya",
        ),
        ("/models/alpha/shared", "alpha/shared"),
        (
            "/models/zen/convaiinnovations/laya",
            "zen/convaiinnovations/laya",
        ),
    ] {
        let (status, body) = get_json(addr, path).await;
        assert_eq!(status, 200, "GET {path} must be 200");
        assert_eq!(body["id"], expected_id, "GET {path} id echo");
        assert_eq!(
            keys_of(&body),
            expected_model_keys(),
            "GET {path} must return one frozen-13-key model object"
        );
        let expected_capacity = capacity_for(expected_id);
        assert_eq!(
            (
                body["context_window"]
                    .as_u64()
                    .expect("context_window number"),
                body["max_output"].as_u64().expect("max_output number"),
            ),
            expected_capacity,
            "GET {path} capacity must match the list entry for {expected_id}"
        );
    }
}

/// J-05 — a bare model name echoes the bare name while its capacity comes from
/// the matched list entry; a miss stays a 3-key error envelope with no capacity
/// keys (§5.3, B14).
#[tokio::test]
async fn j_05_bare_name_echo_and_error_envelope() {
    let state = Arc::new(AppState::new(fixture_http()));
    let addr = spawn_gateway(state.clone()).await;
    let list = state.list_all_models_with_capacity();

    let (status, body) = get_json(addr, "/v1/models/shared").await;
    assert_eq!(status, 200);
    assert_eq!(
        body["id"], "shared",
        "bare name must be echoed as requested"
    );
    // First matching list entry for bare `shared` is `alpha/shared`.
    let alpha_shared = list
        .iter()
        .find(|m| m.id == "alpha/shared")
        .expect("alpha/shared entry");
    assert_eq!(
        (
            body["context_window"].as_u64().expect("number"),
            body["max_output"].as_u64().expect("number"),
        ),
        (alpha_shared.context_window, alpha_shared.max_output),
        "bare-name capacity must come from the matched entry, not a re-lookup"
    );

    let (status, body) = get_json(addr, "/v1/models/definitely-not-a-model").await;
    assert_eq!(status, 404, "unknown model must be 404");
    let envelope = body.as_object().expect("error envelope object");
    assert_eq!(
        envelope.len(),
        1,
        "error body must contain only `error`: {body}"
    );
    let error = body["error"].as_object().expect("error object");
    assert_eq!(
        error.keys().map(String::as_str).collect::<Vec<_>>(),
        vec!["message", "type", "code"],
        "error envelope must keep its frozen 3 keys"
    );
    assert_eq!(body["error"]["code"], "model_not_found");
    assert!(
        body.get("context_window").is_none() && body.get("max_output").is_none(),
        "404 body must not carry capacity keys: {body}"
    );
}

/// J-06 — `GET /models` and `GET /v1/models` are the same handler/envelope/13
/// keys (§5.2).
#[tokio::test]
async fn j_06_models_and_v1_models_are_equivalent() {
    let addr = spawn_gateway(Arc::new(AppState::new(fixture_http()))).await;
    let (v1_status, v1_body) = get_json(addr, "/v1/models").await;
    let (bare_status, bare_body) = get_json(addr, "/models").await;
    assert_eq!(v1_status, 200, "GET /v1/models must be 200");
    assert_eq!(bare_status, 200, "GET /models must be 200");

    assert_eq!(
        bare_body, v1_body,
        "`/models` and `/v1/models` must be byte-equivalent JSON"
    );
    let first = v1_body["data"]
        .as_array()
        .and_then(|data| data.first())
        .expect("data[0]");
    assert_eq!(
        keys_of(first),
        expected_model_keys(),
        "both endpoints must project the frozen 13 keys"
    );
}
