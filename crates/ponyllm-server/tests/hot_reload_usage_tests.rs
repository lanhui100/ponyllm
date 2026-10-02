//! 热重载不归零：`reload_config_with_pools` 按 key id 移植 usage tracker，
//! 未命中 donor 的 key 从持久化快照回退恢复（账号增删历史保留）。

use std::collections::HashMap;
use std::sync::Arc;

use ponyllm_core::pool::{
    ApiKeyEntry, BillingMode, KeyPool, RoutingStrategy,
};
use ponyllm_server::config::ProviderConfig;
use ponyllm_server::{AppState, GatewayConfig};

fn provider(base: &str, model: &str) -> ProviderConfig {
    ProviderConfig {
        rate_limits: None,
        base_url: format!("http://{}/v1", base),
        default_model: model.to_string(),
        strategy: "round_robin".to_string(),
        billing_mode: BillingMode::Metered,
        input_price: 1.0,
        cached_price: 0.5,
        output_price: 2.0,
        models: vec![model.to_string()],
        model_specs: vec![],
        default_protocol: None,
        chat_url: None,
        responses_url: None,
        messages_url: None,
        proxy: None,
        timeout_secs: None,
        ttfb_timeout_secs: None,
    }
}

fn build_pool(provider: &str, key_ids: &[&str]) -> Arc<KeyPool> {
    let pool = Arc::new(KeyPool::new(provider, RoutingStrategy::RoundRobin));
    for id in key_ids {
        pool.add_key(ApiKeyEntry::new(*id, "sk-dummy", 1, 1));
    }
    pool
}

#[tokio::test]
async fn hot_reload_preserves_usage_history_by_key_id_and_restores_from_snapshot() {
    let temp_dir = tempfile::tempdir().unwrap();
    let snapshot_path = temp_dir.path().join("telemetry-snapshot.json");

    let mut gw_config = GatewayConfig::default();
    gw_config.bind_addr = "127.0.0.1:0".to_string();
    gw_config.api_key = String::new();
    gw_config.telemetry_snapshot_path = Some(snapshot_path.to_string_lossy().into_owned());
    let state = Arc::new(AppState::new(gw_config));
    let now_ms = 1_700_000_000_000u64;

    // <caption>1. 初始池 acc-1 + acc-3，均产生用量；acc-1 完成一个 5h 周期。</caption>
    let pool = build_pool("prov_x", &["acc-1", "acc-3"]);
    {
        let key1 = pool.snapshot_keys().into_iter().find(|k| k.id == "acc-1").unwrap();
        // 5h 完整周期：1.0 → 消耗 → 0.8 → 再消耗 → 跳回 1.0（打满重置）。
        key1.usage_tracker.observe_upstream_probe(now_ms - 3600 * 1000, 1.0);
        key1.usage_tracker.record_tokens(now_ms, 10_000, 5_000, 1_000);
        key1.usage_tracker.observe_upstream_probe(now_ms, 0.8);
        key1.usage_tracker.record_tokens(now_ms + 1800 * 1000, 2_000, 1_000, 0);
        key1.usage_tracker.observe_upstream_probe(now_ms + 3600 * 1000, 1.0);
        let key3 = pool.snapshot_keys().into_iter().find(|k| k.id == "acc-3").unwrap();
        key3.usage_tracker.record_tokens(now_ms, 3_000, 1_000, 0);
    }
    state.register_pool("prov_x", pool.clone());
    // 持久化快照（含 acc-1 周期历史 + acc-3 用量）。
    state.save_telemetry_snapshot().unwrap();

    let mut cfg1 = GatewayConfig::default();
    cfg1.bind_addr = "127.0.0.1:0".to_string();
    cfg1.providers.insert("prov_x".to_string(), provider("127.0.0.1:9", "m1"));

    // <caption>2. 热重载 A：acc-3 被移除，只剩 acc-1（同 id 重建 → 移植 tracker）。</caption>
    let new_pool_a = build_pool("prov_x", &["acc-1"]);
    let mut pools_a = HashMap::new();
    pools_a.insert("prov_x".to_string(), new_pool_a.clone());
    state.reload_config_with_pools(cfg1.clone(), pools_a);

    let live = state.get_pool("prov_x").unwrap();
    let k1 = live
        .snapshot_keys()
        .into_iter()
        .find(|k| k.id == "acc-1")
        .unwrap();
    let usage = k1.usage_tracker.query_window(now_ms, ponyllm_core::pool::usage::FIVE_HOURS_MS);
    assert_eq!(
        usage.total_tokens, 15_000,
        "acc-1 窗口用量在热重载后保留（tracker 已移植）"
    );
    let est = k1.usage_tracker.estimate_capacity(now_ms, None);
    assert!(
        est.completed_5h_stats.is_some(),
        "acc-1 完整周期历史在热重载后保留"
    );

    // <caption>3. 热重载 B：acc-3 重新加入（无 in-memory donor）→ 从快照文件回退恢复。</caption>
    let new_pool_b = build_pool("prov_x", &["acc-1", "acc-3"]);
    let mut pools_b = HashMap::new();
    pools_b.insert("prov_x".to_string(), new_pool_b);
    state.reload_config_with_pools(cfg1, pools_b);

    let live2 = state.get_pool("prov_x").unwrap();
    let k3 = live2
        .snapshot_keys()
        .into_iter()
        .find(|k| k.id == "acc-3")
        .unwrap();
    let usage3 = k3.usage_tracker.query_window(now_ms, ponyllm_core::pool::usage::FIVE_HOURS_MS);
    assert_eq!(
        usage3.total_tokens, 4_000,
        "acc-3 重新加入后从快照恢复用量历史（不归零）"
    );
    let k1_after = live2
        .snapshot_keys()
        .into_iter()
        .find(|k| k.id == "acc-1")
        .unwrap();
    let est1_after = k1_after.usage_tracker.estimate_capacity(now_ms, None);
    assert!(
        est1_after.completed_5h_stats.is_some(),
        "acc-1 周期历史在二次热重载后仍保留"
    );
}