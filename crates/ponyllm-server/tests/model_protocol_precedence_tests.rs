#![allow(clippy::field_reassign_with_default)]

//! Black-box contract tests for upstream protocol precedence.
//!
//! Production incident (opencode-zen / `muse-spark-1.3-contributor-free`):
//! the provider declared `default_protocol = "responses"` while the very same
//! provider's `model_configs` entry declared `protocol = "anthropic"`. Model
//! level wins, so every request silently went to `.../v1/messages` instead of
//! `.../v1/responses` and the gateway surfaced 503.
//!
//! These tests pin the *current, intended* precedence chain so a future
//! "fix" cannot silently reorder it again:
//!   request header override (`x-pony-protocol`)
//!     > inbound protocol with an explicit per-protocol endpoint override
//!     > model-level `ModelSpec.protocol`
//!     > provider-level `ProviderConfig.default_protocol`
//!     > legacy URL heuristic (`infer_legacy_protocol`)
//!
//! Pure black box: everything below goes through public APIs
//! (`AppState::resolve_routed_targets*`, `ParsedRequestModel::parse`) and only
//! reads `RoutedTarget`'s public fields. No implementation internals are
//! touched and no HTTP mock is required.

use ponyllm_config::AuthMode;
use ponyllm_core::pool::*;
use ponyllm_server::routes::models::ParsedRequestModel;
use ponyllm_server::{AppState, GatewayConfig, ModelSpec, ProviderConfig};

/// Model used throughout: a 1M-capable spec so `[1m]` requests survive the
/// context-capacity filter (`is_context_capacity_compatible`).
const MODEL: &str = "muse-spark-1.3-contributor-free";

/// Provider under test (mirrors the real `opencode-zen` provider name).
const PROVIDER: &str = "opencode-zen";

/// Fully-specified provider builder.
///
/// * `default_protocol` -> `ProviderConfig::default_protocol` (provider level)
/// * `model_protocol`   -> `ModelSpec.protocol` (model level)
/// * `with_model_spec`  -> emit a `ModelSpec` at all; when `false` the model
///   has *no* spec entry and therefore no model-level protocol declaration.
fn provider(
    base: &str,
    default_protocol: Option<UpstreamProtocol>,
    model_protocol: Option<UpstreamProtocol>,
    with_model_spec: bool,
    endpoint: Option<(UpstreamProtocol, &str)>,
) -> ProviderConfig {
    let mut chat_url = None;
    let mut responses_url = None;
    let mut messages_url = None;
    if let Some((proto, url)) = endpoint {
        match proto {
            UpstreamProtocol::Chat => chat_url = Some(url.to_string()),
            UpstreamProtocol::Responses => responses_url = Some(url.to_string()),
            UpstreamProtocol::Anthropic => messages_url = Some(url.to_string()),
            // Antigravity / Systemone have no per-protocol endpoint override
            // (see `ProviderConfig::endpoint_base_for`); call sites must not
            // request one.
            _ => panic!("protocol {proto:?} has no per-protocol endpoint override"),
        }
    }

    ProviderConfig {
        base_url: base.to_string(),
        default_model: MODEL.to_string(),
        strategy: "round_robin".to_string(),
        billing_mode: BillingMode::Metered,
        input_price: 0.1,
        cached_price: 0.01,
        output_price: 0.2,
        models: vec![MODEL.to_string()],
        model_specs: if with_model_spec {
            vec![ModelSpec {
                name: MODEL.to_string(),
                tier: ModelTier::Standard,
                context_window: "1M".to_string(),
                max_output: "32K".to_string(),
                input_types: vec!["text".to_string()],
                output_types: vec!["text".to_string()],
                protocol: model_protocol,
                ..Default::default()
            }]
        } else {
            vec![]
        },
        default_protocol,
        chat_url,
        responses_url,
        messages_url,
        proxy: None,
        ..Default::default()
    }
}

/// Build an `AppState` holding exactly one provider under [`PROVIDER`].
fn state_with(p: ProviderConfig) -> AppState {
    let mut config = GatewayConfig::default();
    config.auth_mode = AuthMode::Open;
    config.providers.insert(PROVIDER.to_string(), p);
    AppState::new(config)
}

/// A1: model-level `protocol` beats provider-level `default_protocol`.
///
/// This is the exact production shape that caused the 503: provider said
/// `responses`, model said `anthropic`, upstream URL became `/v1/messages`.
#[test]
fn a1_model_level_protocol_beats_provider_default_protocol() {
    // Arrange
    let state = state_with(provider(
        "https://opencode.ai/zen/v1",
        Some(UpstreamProtocol::Responses),   // provider level
        Some(UpstreamProtocol::Anthropic),  // model level -> must win
        true,
        None,
    ));

    // Act
    let targets = state
        .resolve_routed_targets(&ParsedRequestModel::parse(MODEL), None)
        .expect("model resolves to at least one routed target");

    // Assert
    assert_eq!(targets.len(), 1, "exactly one provider serves {MODEL}");
    assert_eq!(targets[0].upstream_protocol, UpstreamProtocol::Anthropic);
    assert_eq!(targets[0].provider_name, PROVIDER);
    // A model-level protocol must not leak a provider endpoint override it
    // never asked for.
    assert_eq!(targets[0].endpoint_base, None);
    assert_eq!(targets[0].messages_url(), "https://opencode.ai/zen/v1/messages");
}

/// A2: the reverse direction — model-level `responses` beats provider-level
/// `anthropic`. Guards against a half-applied inversion of the precedence.
#[test]
fn a2_model_level_responses_beats_provider_level_anthropic() {
    // Arrange
    let state = state_with(provider(
        "https://opencode.ai/zen/v1",
        Some(UpstreamProtocol::Anthropic),
        Some(UpstreamProtocol::Responses),
        true,
        None,
    ));

    // Act
    let targets = state
        .resolve_routed_targets(&ParsedRequestModel::parse(MODEL), None)
        .expect("model resolves to at least one routed target");

    // Assert
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].upstream_protocol, UpstreamProtocol::Responses);
    assert_eq!(targets[0].endpoint_base, None);
    assert_eq!(targets[0].responses_url(), "https://opencode.ai/zen/v1/responses");
}

/// A3: a model spec *without* `protocol` inherits the provider default.
/// The distinction from A1 is the whole point: presence of a `ModelSpec`
/// must not by itself shadow the provider default.
#[test]
fn a3_model_without_protocol_inherits_provider_default_protocol() {
    // Arrange: model spec exists, but declares no protocol.
    let state = state_with(provider(
        "https://opencode.ai/zen/v1",
        Some(UpstreamProtocol::Responses),
        None, // model level: undeclared
        true,
        None,
    ));

    // Act
    let targets = state
        .resolve_routed_targets(&ParsedRequestModel::parse(MODEL), None)
        .expect("model resolves to at least one routed target");

    // Assert
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].upstream_protocol, UpstreamProtocol::Responses);
}

/// A3b: same inheritance rule with the opposite provider default, so the test
/// cannot pass by a hard-coded constant.
#[test]
fn a3b_model_without_protocol_inherits_provider_default_anthropic() {
    // Arrange
    let state = state_with(provider(
        "https://opencode.ai/zen/v1",
        Some(UpstreamProtocol::Anthropic),
        None,
        true,
        None,
    ));

    // Act
    let targets = state
        .resolve_routed_targets(&ParsedRequestModel::parse(MODEL), None)
        .expect("model resolves to at least one routed target");

    // Assert
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].upstream_protocol, UpstreamProtocol::Anthropic);
}

/// A4: neither level declares a protocol -> legacy URL heuristic
/// (`infer_legacy_protocol`): an `anthropic` path segment wins, otherwise an
/// `anthropic` provider name outside a `/v1/chat` base wins, else Chat.
#[test]
fn a4_falls_back_to_legacy_url_heuristic_when_nothing_is_declared() {
    // --- Arrange / Act: `anthropic` in the base URL path -> Anthropic.
    let ant_by_url = state_with(provider(
        "https://api.example.com/anthropic",
        None, // provider level: undeclared
        None, // model level: undeclared
        false, // no ModelSpec at all
        None,
    ));
    let ant_targets = ant_by_url
        .resolve_routed_targets(&ParsedRequestModel::parse(MODEL), None)
        .expect("heuristic provider resolves");
    // Assert
    assert_eq!(ant_targets.len(), 1);
    assert_eq!(
        ant_targets[0].upstream_protocol,
        UpstreamProtocol::Anthropic,
        "base_url containing an `anthropic` segment must select the Anthropic wire protocol"
    );

    // --- Arrange / Act: plain URL -> Chat.
    let chat_by_url = state_with(provider(
        "https://api.example.com/v1",
        None,
        None,
        false,
        None,
    ));
    let chat_targets = chat_by_url
        .resolve_routed_targets(&ParsedRequestModel::parse(MODEL), None)
        .expect("heuristic provider resolves");
    // Assert
    assert_eq!(chat_targets.len(), 1);
    assert_eq!(
        chat_targets[0].upstream_protocol,
        UpstreamProtocol::Chat,
        "a non-anthropic base_url must fall back to the OpenAI Chat wire protocol"
    );
}

/// A4b: a model-level declaration outranks the URL heuristic — i.e. the
/// heuristic is the *last* resort, not a co-equal fallback.
#[test]
fn a4b_model_level_protocol_outranks_legacy_url_heuristic() {
    // Arrange: URL screams `anthropic`, model says `responses`.
    let state = state_with(provider(
        "https://api.example.com/anthropic",
        None,
        Some(UpstreamProtocol::Responses),
        true,
        None,
    ));

    // Act
    let targets = state
        .resolve_routed_targets(&ParsedRequestModel::parse(MODEL), None)
        .expect("model resolves to at least one routed target");

    // Assert
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].upstream_protocol, UpstreamProtocol::Responses);
}

/// A5: the `[1m]` suffix is request syntax, not part of the model id. After
/// stripping it, protocol resolution must still find the model-level spec
/// (and the upstream payload must carry the bare name, never `[1m]`).
#[test]
fn a5_one_m_suffix_is_stripped_and_still_hits_model_level_protocol() {
    // Arrange
    let state = state_with(provider(
        "https://opencode.ai/zen/v1",
        Some(UpstreamProtocol::Responses),  // provider level
        Some(UpstreamProtocol::Anthropic), // model level -> must still win
        true,
        None,
    ));
    let parsed = ParsedRequestModel::parse("muse-spark-1.3-contributor-free[1m]");

    // Act
    let targets = state
        .resolve_routed_targets(&parsed, None)
        .expect("[1m] request resolves to at least one routed target");

    // Assert — parsing
    assert_eq!(parsed.raw_requested_model, "muse-spark-1.3-contributor-free[1m]");
    assert_eq!(parsed.clean_model_name, "muse-spark-1.3-contributor-free");
    assert!(parsed.is_1m_context);
    // Assert — resolution
    assert_eq!(targets.len(), 1, "[1m] must not fan out or drop the candidate");
    assert_eq!(targets[0].provider_name, PROVIDER);
    assert_eq!(targets[0].physical_model, "muse-spark-1.3-contributor-free");
    assert!(
        !targets[0].physical_model.contains("[1m]"),
        "upstream model id must never carry the [1m] request suffix, got {:?}",
        targets[0].physical_model
    );
    assert_eq!(
        targets[0].upstream_protocol,
        UpstreamProtocol::Anthropic,
        "model-level protocol must still be selected after [1m] stripping"
    );
}

/// A5b: `[1m]` + strategy suffix combined, still stripped and still
/// model-level-driven. Guards the tokenizer against a `:`-only fix.
#[test]
fn a5b_one_m_suffix_with_strategy_tag_still_resolves_model_level_protocol() {
    // Arrange
    let state = state_with(provider(
        "https://opencode.ai/zen/v1",
        Some(UpstreamProtocol::Responses),
        Some(UpstreamProtocol::Anthropic),
        true,
        None,
    ));
    let parsed = ParsedRequestModel::parse("muse-spark-1.3-contributor-free[1m]:economy");

    // Act
    let targets = state
        .resolve_routed_targets(&parsed, None)
        .expect("[1m]:economy request resolves");

    // Assert
    assert_eq!(parsed.clean_model_name, "muse-spark-1.3-contributor-free");
    assert_eq!(parsed.strategy_override, Some(GatewayRoutingStrategy::Economy));
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].physical_model, "muse-spark-1.3-contributor-free");
    assert_eq!(targets[0].upstream_protocol, UpstreamProtocol::Anthropic);
}

/// A6: the *existing* "inbound protocol + explicit per-protocol endpoint
/// override beats the model-level declaration" branch of
/// `resolve_effective_protocol`.
///
/// This is pre-existing deliberate behavior (native passthrough is preferred
/// over translation when the provider publishes a dedicated endpoint for the
/// inbound protocol). It is NOT the defect under repair, so this test locks
/// the current behavior rather than changing it. If a future change intends
/// to drop this branch, this test is the tripwire that must be updated
/// deliberately.
#[test]
fn a6_inbound_protocol_with_explicit_endpoint_beats_model_level_declaration() {
    // Arrange: model declares `responses`; provider publishes an explicit
    // `messages_url`; the request arrives on the Anthropic inbound entry.
    let state = state_with(provider(
        "https://opencode.ai/zen/v1",
        Some(UpstreamProtocol::Responses),
        Some(UpstreamProtocol::Responses), // model level
        true,
        Some((UpstreamProtocol::Anthropic, "https://native.example.com/anthropic/v1")),
    ));

    // Act
    let targets = state
        .resolve_routed_targets_with_prompt_and_protocol(
            &ParsedRequestModel::parse(MODEL),
            None,
            None,
            None,                               // no x-pony-protocol header
            Some(UpstreamProtocol::Anthropic), // inbound entry protocol
        )
        .expect("model resolves to at least one routed target");

    // Assert — locked current behavior
    assert_eq!(targets.len(), 1);
    assert_eq!(
        targets[0].upstream_protocol,
        UpstreamProtocol::Anthropic,
        "inbound protocol with an explicit endpoint override currently outranks the model-level declaration"
    );
    assert_eq!(
        targets[0].endpoint_base.as_deref(),
        Some("https://native.example.com/anthropic/v1")
    );
    assert_eq!(
        targets[0].messages_url(),
        "https://native.example.com/anthropic/v1/messages"
    );
}

/// A6b: the complementary half of the A6 branch — when the provider has *no*
/// endpoint override for the inbound protocol, the model-level declaration
/// must survive. Without this, a naive "inbound always wins" change would
/// silently regress production routing.
#[test]
fn a6b_inbound_protocol_without_endpoint_override_does_not_override_model_level() {
    // Arrange: same as A6 but with no `messages_url` configured.
    let state = state_with(provider(
        "https://opencode.ai/zen/v1",
        Some(UpstreamProtocol::Responses),
        Some(UpstreamProtocol::Responses),
        true,
        None,
    ));

    // Act
    let targets = state
        .resolve_routed_targets_with_prompt_and_protocol(
            &ParsedRequestModel::parse(MODEL),
            None,
            None,
            None,
            Some(UpstreamProtocol::Anthropic),
        )
        .expect("model resolves to at least one routed target");

    // Assert
    assert_eq!(targets.len(), 1);
    assert_eq!(
        targets[0].upstream_protocol,
        UpstreamProtocol::Responses,
        "an inbound protocol without a configured endpoint must not hijack a declared model-level protocol"
    );
    assert_eq!(targets[0].endpoint_base, None);
}

/// A6c: the explicit request header (`x-pony-protocol`) still outranks the
/// model-level declaration, and it composes with the A6 endpoint branch.
#[test]
fn a6c_explicit_request_header_override_outranks_model_level_declaration() {
    // Arrange
    let state = state_with(provider(
        "https://opencode.ai/zen/v1",
        Some(UpstreamProtocol::Responses),
        Some(UpstreamProtocol::Responses),
        true,
        None,
    ));

    // Act
    let targets = state
        .resolve_routed_targets_with_prompt_and_protocol(
            &ParsedRequestModel::parse(MODEL),
            None,
            None,
            Some(UpstreamProtocol::Chat), // x-pony-protocol: chat
            None,
        )
        .expect("model resolves to at least one routed target");

    // Assert
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].upstream_protocol, UpstreamProtocol::Chat);
    assert_eq!(targets[0].endpoint_base, None);
}