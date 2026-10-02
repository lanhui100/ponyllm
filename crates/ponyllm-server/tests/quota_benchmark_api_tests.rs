//! Pool-level cycle benchmark endpoint tests (`GET /api/admin/quota/benchmark`).
//!
//! 门禁：端点可达且只读；无快照时返回全零视图；快照写入后返回持久化累计基准
//! （观测数/均值/打满周期数）；重复保存幂等；未授权 401。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use ponyllm_core::pool::usage::KeyUsageStateSnapshot;
use ponyllm_server::telemetry_snapshot::{
    save_snapshot_with_live_cycles, TelemetrySnapshot,
};
use ponyllm_server::{create_app, AppState, GatewayConfig};
use reqwest::StatusCode;

struct BenchmarkHarness {
    addr: SocketAddr,
    api_key: String,
    snapshot_path: PathBuf,
    _temp_dir: tempfile::TempDir,
}

impl BenchmarkHarness {
    async fn new() -> Self {
        let temp_dir = tempfile::tempdir().unwrap();
        let snapshot_path = temp_dir.path().join("telemetry-snapshot.json");

        let mut gw_config = GatewayConfig::default();
        gw_config.bind_addr = "127.0.0.1:8080".to_string();
        gw_config.api_key = "bench-test-secret".to_string();
        gw_config.admin_write_enabled = false;
        gw_config.telemetry_snapshot_path = Some(snapshot_path.to_string_lossy().into_owned());

        let state = Arc::new(AppState::new(gw_config));
        let app = create_app(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        Self {
            addr,
            api_key: "bench-test-secret".to_string(),
            snapshot_path,
            _temp_dir: temp_dir,
        }
    }

    fn auth(&self, b: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        b.header("Authorization", format!("Bearer {}", self.api_key))
    }

    fn write_snapshot(&self, usages: HashMap<String, KeyUsageStateSnapshot>) {
        let mut snap = TelemetrySnapshot::default();
        snap.key_usages = usages;
        save_snapshot_with_live_cycles(&self.snapshot_path, &snap, HashMap::new()).unwrap();
    }
}

#[tokio::test]
async fn benchmark_endpoint_requires_auth() {
    let h = BenchmarkHarness::new().await;
    let resp = reqwest::Client::new()
        .get(format!("http://{}/api/admin/quota/benchmark", h.addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn benchmark_endpoint_returns_zero_view_without_snapshot() {
    let h = BenchmarkHarness::new().await;
    let resp = h
        .auth(
            reqwest::Client::new().get(format!(
                "http://{}/api/admin/quota/benchmark",
                h.addr
            )),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["kind_5h"]["observations"], 0);
    assert_eq!(body["kind_weekly"]["observations"], 0);
    assert_eq!(body["kind_monthly"]["observations"], 0);
}

#[tokio::test]
async fn benchmark_endpoint_serves_persisted_archive() {
    let h = BenchmarkHarness::new().await;

    let period = ponyllm_core::pool::usage::FIVE_HOURS_MS;
    let aligned_now = (1_700_000_000_000u64 / period) * period + period;
    let mut usages = HashMap::new();
    usages.insert(
        "acc-1".to_string(),
        KeyUsageStateSnapshot {
            slices: vec![ponyllm_core::pool::usage::UsageSlice {
                timestamp_ms: aligned_now - period + 1_000,
                prompt_tokens: 8_000,
                completion_tokens: 2_000,
                cached_tokens: 500,
                requests: 4,
                ..Default::default()
            }],
            ..Default::default()
        },
    );
    h.write_snapshot(usages);

    let resp = h
        .auth(
            reqwest::Client::new().get(format!(
                "http://{}/api/admin/quota/benchmark",
                h.addr
            )),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["kind_5h"]["observations"], 1);
    assert_eq!(body["kind_5h"]["total_tokens"], 10_000);
    assert_eq!(body["kind_5h"]["avg_tokens"], 10_000);
    assert_eq!(body["kind_5h"]["requests"], 4);
    assert_eq!(body["kind_5h"]["completed_cycles"], 0);
    assert!(body["persisted_at_ms"].as_u64().unwrap() > 0);
}
