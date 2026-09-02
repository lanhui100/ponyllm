use ponyllm_core::pool::{
    is_context_capacity_compatible, parse_context_capacity_tokens, BillingMode,
    GatewayRoutingStrategy, ModelTier, PricingConfig, QuotaLease,
};
use std::str::FromStr;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

#[test]
fn test_billing_mode_default_and_serde() {
    assert_eq!(BillingMode::default(), BillingMode::Metered);
    let serialized = serde_json::to_string(&BillingMode::Metered).unwrap();
    assert_eq!(serialized, "\"metered\"");
    let deserialized: BillingMode = serde_json::from_str("\"plan\"").unwrap();
    assert_eq!(deserialized, BillingMode::Plan);
}

#[test]
fn test_gateway_routing_strategy_default_and_parsing() {
    // Default must be Economy
    assert_eq!(GatewayRoutingStrategy::default(), GatewayRoutingStrategy::Economy);

    // Parsing various forms
    assert_eq!(GatewayRoutingStrategy::from_str("economy").unwrap(), GatewayRoutingStrategy::Economy);
    assert_eq!(GatewayRoutingStrategy::from_str("cheap").unwrap(), GatewayRoutingStrategy::Economy);
    assert_eq!(GatewayRoutingStrategy::from_str("e").unwrap(), GatewayRoutingStrategy::Economy);

    assert_eq!(GatewayRoutingStrategy::from_str("speed").unwrap(), GatewayRoutingStrategy::Speed);
    assert_eq!(GatewayRoutingStrategy::from_str("fastest").unwrap(), GatewayRoutingStrategy::Speed);
    assert_eq!(GatewayRoutingStrategy::from_str("s").unwrap(), GatewayRoutingStrategy::Speed);

    assert_eq!(GatewayRoutingStrategy::from_str("reliable").unwrap(), GatewayRoutingStrategy::Reliable);
    assert_eq!(GatewayRoutingStrategy::from_str("ha").unwrap(), GatewayRoutingStrategy::Reliable);
    assert_eq!(GatewayRoutingStrategy::from_str("r").unwrap(), GatewayRoutingStrategy::Reliable);

    assert_eq!(GatewayRoutingStrategy::from_str("balanced").unwrap(), GatewayRoutingStrategy::Balanced);
    assert_eq!(GatewayRoutingStrategy::from_str("b").unwrap(), GatewayRoutingStrategy::Balanced);

    assert!(GatewayRoutingStrategy::from_str("unknown").is_err());
}

#[test]
fn test_model_tier_shorthand_and_parsing() {
    // Default must be Flagship
    assert_eq!(ModelTier::default(), ModelTier::Flagship);

    // Shorthand and full names
    assert_eq!(ModelTier::from_str("F").unwrap(), ModelTier::Flagship);
    assert_eq!(ModelTier::from_str("flagship").unwrap(), ModelTier::Flagship);
    assert_eq!(ModelTier::Flagship.shorthand(), "F");

    assert_eq!(ModelTier::from_str("S").unwrap(), ModelTier::Standard);
    assert_eq!(ModelTier::from_str("standard").unwrap(), ModelTier::Standard);
    assert_eq!(ModelTier::Standard.shorthand(), "S");

    assert_eq!(ModelTier::from_str("L").unwrap(), ModelTier::Light);
    assert_eq!(ModelTier::from_str("light").unwrap(), ModelTier::Light);
    assert_eq!(ModelTier::Light.shorthand(), "L");

    // Tier ranking: Flagship > Standard > Light
    assert!(ModelTier::Flagship > ModelTier::Standard);
    assert!(ModelTier::Standard > ModelTier::Light);

    assert!(ModelTier::from_str("Z").is_err());
}

#[test]
fn test_context_capacity_parsing_and_monotonicity() {
    assert_eq!(parse_context_capacity_tokens("1M"), 1048576);
    assert_eq!(parse_context_capacity_tokens("1024K"), 1048576);
    assert_eq!(parse_context_capacity_tokens("256K"), 262144);
    assert_eq!(parse_context_capacity_tokens("128K"), 131072);
    assert_eq!(parse_context_capacity_tokens("32K"), 32768);
    assert_eq!(parse_context_capacity_tokens("8K"), 8192);

    // Only allow equal or increasing capacity (small -> large)
    assert!(is_context_capacity_compatible("128K", "1M"));
    assert!(is_context_capacity_compatible("128K", "128K"));
    assert!(is_context_capacity_compatible("32K", "128K"));

    // Strictly disallow reverse (large -> small), which would exceed context
    assert!(!is_context_capacity_compatible("1M", "128K"));
    assert!(!is_context_capacity_compatible("256K", "32K"));
}

#[test]
fn test_pricing_config_and_is_free_precision() {
    let default_pricing = PricingConfig::default();
    assert_eq!(default_pricing.input_price, 0.50);
    assert_eq!(default_pricing.cached_price, 0.25);
    assert_eq!(default_pricing.output_price, 1.00);
    assert!(!default_pricing.is_free());

    let free_pricing = PricingConfig {
        input_price: 0.0,
        cached_price: 0.0,
        output_price: 0.0,
    };
    assert!(free_pricing.is_free());
    assert_eq!(free_pricing.estimate_cost(100_000, true, 1_000), 0.0);

    let paid_pricing = PricingConfig {
        input_price: 0.14,
        cached_price: 0.014,
        output_price: 0.28,
    };
    // 1M input cache miss (0.14) + 1M output (0.28) = 0.42
    let cost_miss = paid_pricing.estimate_cost(1_000_000, false, 1_000_000);
    assert!((cost_miss - 0.42).abs() < 1e-6);

    // 1M input cache hit (0.014) + 1M output (0.28) = 0.294
    let cost_hit = paid_pricing.estimate_cost(1_000_000, true, 1_000_000);
    assert!((cost_hit - 0.294).abs() < 1e-6);
}

#[test]
fn test_quota_lease_atomic_preacquire_and_drop_rollback() {
    let quota = Arc::new(AtomicI64::new(1));

    // 1. Acquire lease successfully
    {
        let lease = QuotaLease::try_acquire(&quota);
        assert!(lease.is_some());
        assert_eq!(quota.load(Ordering::SeqCst), 0);

        // Another concurrent acquire will fail (remaining == 0)
        let lease_fail = QuotaLease::try_acquire(&quota);
        assert!(lease_fail.is_none());

        // Drop lease without commit -> should automatically rollback quota
    }
    assert_eq!(quota.load(Ordering::SeqCst), 1);

    // 2. Acquire and commit -> should keep quota consumed
    {
        let lease = QuotaLease::try_acquire(&quota).unwrap();
        assert_eq!(quota.load(Ordering::SeqCst), 0);
        lease.commit();
    }
    assert_eq!(quota.load(Ordering::SeqCst), 0);
}
