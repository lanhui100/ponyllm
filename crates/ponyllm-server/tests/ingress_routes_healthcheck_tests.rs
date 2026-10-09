//! Phase 3 IngressRoute 主动探活配置校验（T3 阶段3 配置契约）。
//!
//! 断言 `deploy/ponyllm-ingress-routes.yaml` 中每一个 IngressRoute 的每一个
//! 后端 service 都携带 Traefik 原生主动探活 `healthCheck`（path=/health，
//! intervalSeconds ≤ 3 且 timeoutSeconds ≥ 1），使边缘能在 ~3 秒内自发切除
//! 失效端点 —— 即使控制面宕机导致 kubelet 无法更新 endpoints，前端也不卡死。
//!
//! 这是机械校验（非零退出），与 `scripts/phase3-verify.sh` 的集群侧验证互补。

use serde::Deserialize;
use serde_yaml::Value;

const INGRESS_ROUTES_YAML: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../deploy/ponyllm-ingress-routes.yaml"
);

fn load_ingress_routes() -> Vec<Value> {
    let raw = std::fs::read_to_string(INGRESS_ROUTES_YAML)
        .expect("deploy/ponyllm-ingress-routes.yaml must exist");
    serde_yaml::Deserializer::from_str(&raw)
        .map(Value::deserialize)
        .collect::<Result<Vec<Value>, _>>()
        .unwrap_or_else(|e| panic!("invalid YAML in {}: {}", INGRESS_ROUTES_YAML, e))
}

#[test]
fn every_ingress_route_service_carries_active_health_check() {
    let docs = load_ingress_routes();
    let ingress_routes: Vec<&Value> = docs
        .iter()
        .filter(|d| d["kind"].as_str() == Some("IngressRoute"))
        .collect();
    assert!(
        !ingress_routes.is_empty(),
        "must contain at least one IngressRoute document"
    );

    let mut services_checked = 0usize;
    for route in &ingress_routes {
        let name = route["metadata"]["name"].as_str().unwrap_or("<unnamed>");
        let routes = route["spec"]["routes"]
            .as_sequence()
            .expect("IngressRoute spec.routes must be a list");
        for (i, r) in routes.iter().enumerate() {
            let svcs = r["services"]
                .as_sequence()
                .expect(&format!("route #{i} of {name} must have services"));
            for svc in svcs {
                let svc_name = svc["name"].as_str().unwrap_or("<unnamed>");
                let hc = &svc["healthCheck"];
                assert!(
                    !hc.is_null(),
                    "service '{svc_name}' (route #{i} of {name}) must have healthCheck"
                );
                assert_eq!(
                    hc["path"].as_str(),
                    Some("/health"),
                    "service '{svc_name}' healthCheck.path must be /health"
                );
                let interval = hc["intervalSeconds"].as_u64().unwrap_or_else(|| {
                    panic!("service '{svc_name}' healthCheck.intervalSeconds must be an integer")
                });
                assert!(
                    interval <= 3,
                    "service '{svc_name}' healthCheck.intervalSeconds={interval} must be ≤ 3 (edge must cut a dead endpoint within ~3s)"
                );
                let timeout = hc["timeoutSeconds"].as_u64().unwrap_or_else(|| {
                    panic!("service '{svc_name}' healthCheck.timeoutSeconds must be an integer")
                });
                assert!(
                    timeout >= 1,
                    "service '{svc_name}' healthCheck.timeoutSeconds={timeout} must be ≥ 1"
                );
                services_checked += 1;
            }
        }
    }
    // dev/preprod/proserver/tencent appear in multiple weighted routes; the
    // contract counts every backend occurrence. Sanity floor: at least the
    // ACME (pod-service) + 3 weighted routes + admin route occurrences.
    assert!(
        services_checked >= 6,
        "expected at least 6 service occurrences with healthCheck, found {services_checked}"
    );
}

/// ACME challenge routing must stay functional: the challenge service also
/// carries a health check pointing at the same liveness path.
#[test]
fn acme_challenge_services_have_health_check() {
    let docs = load_ingress_routes();
    let acme_services: Vec<&Value> = docs
        .iter()
        .filter(|d| d["kind"].as_str() == Some("IngressRoute"))
        .flat_map(|d| {
            d["spec"]["routes"]
                .as_sequence()
                .map(|seq| seq.iter().collect::<Vec<_>>())
                .unwrap_or_default()
                .into_iter()
        })
        .filter(|r| {
            r["match"]
                .as_str()
                .map(|m| m.contains("acme-challenge"))
                .unwrap_or(false)
        })
        .flat_map(|r| {
            r["services"]
                .as_sequence()
                .map(|seq| seq.iter().collect::<Vec<_>>())
                .unwrap_or_default()
                .into_iter()
        })
        .collect();
    assert!(
        !acme_services.is_empty(),
        "must have ACME challenge services"
    );
    for svc in acme_services {
        assert_eq!(
            svc["healthCheck"]["path"].as_str(),
            Some("/health"),
            "ACME challenge service '{}' must carry /health healthCheck",
            svc["name"].as_str().unwrap_or("<unnamed>")
        );
    }
}
