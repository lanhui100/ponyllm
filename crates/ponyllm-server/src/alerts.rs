//! Proactive performance alerting (TTFT / stream stall / retry burst).
//!
//! Scope (see `TASK-2026-09-29-alerting-handover.md` §4/§7): **observability only**.
//! Nothing here touches routing, retry counts, timeouts, quota or proxy config.
//!
//! Design: threshold evaluation + cooldown live in [`AlertEvaluator`] (pure,
//! deterministic, unit-testable with an injected clock). Delivery goes through
//! the [`AlertTransport`] trait so tests can record instead of POST. The HTTP
//! transport never blocks the request path: it hands the POST to a detached
//! task and counts failures.
//!
//! Hook point: [`AlertProjection`] is an [`EventBus`] projection, so it reuses
//! the existing `TelemetryStream` sampling (`StreamCompleted.flow` already
//! carries `ttft_ms` / `max_gap_ms`) instead of re-measuring anything.

use parking_lot::Mutex;
use ponyllm_core::telemetry::{EventEnvelope, GatewayEvent, Projection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Env override for the webhook URL. Mirrors pproxy's `PPROXY_ALERT_WEBHOOK_URL`
/// semantics: empty/unset means "log only, never push".
pub const WEBHOOK_ENV: &str = "PONYLLM_ALERT_WEBHOOK_URL";

/// Config block (`[alerts]` in the gateway toml). Every threshold is
/// configurable; defaults match the handover spec.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertConfig {
    /// Master switch. Default `false`: alerting is opt-in observability.
    #[serde(default)]
    pub enabled: bool,
    /// Webhook target. Empty/None → structured log only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhook_url: Option<String>,
    /// `ttft_slow` threshold in ms. Strictly greater triggers (exactly 20000 does not).
    #[serde(default = "default_ttft_slow_ms")]
    pub ttft_slow_ms: f64,
    /// `stream_stall` warning threshold (single inter-chunk gap), ms.
    #[serde(default = "default_stall_gap_ms")]
    pub stream_stall_gap_ms: f64,
    /// `stream_stall` critical threshold (worst observed silence), ms.
    #[serde(default = "default_stall_critical_ms")]
    pub stream_stall_critical_ms: f64,
    /// `retry_burst` warning threshold (transparent retries per request).
    #[serde(default = "default_retry_warning")]
    pub retry_burst_warning: u32,
    /// `retry_burst` critical threshold.
    #[serde(default = "default_retry_critical")]
    pub retry_burst_critical: u32,
    /// Per-event cooldown window in seconds; same-kind events inside the window
    /// merge into one push and the suppressed count lands in `message`.
    #[serde(default = "default_cooldown_secs")]
    pub cooldown_secs: u64,
}

fn default_ttft_slow_ms() -> f64 {
    20_000.0
}
fn default_stall_gap_ms() -> f64 {
    10_000.0
}
fn default_stall_critical_ms() -> f64 {
    60_000.0
}
fn default_retry_warning() -> u32 {
    3
}
fn default_retry_critical() -> u32 {
    10
}
fn default_cooldown_secs() -> u64 {
    60
}

impl Default for AlertConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            webhook_url: None,
            ttft_slow_ms: default_ttft_slow_ms(),
            stream_stall_gap_ms: default_stall_gap_ms(),
            stream_stall_critical_ms: default_stall_critical_ms(),
            retry_burst_warning: default_retry_warning(),
            retry_burst_critical: default_retry_critical(),
            cooldown_secs: default_cooldown_secs(),
        }
    }
}

impl AlertConfig {
    /// Resolve the effective webhook URL: config first, env override wins when
    /// non-empty (ops can rotate the endpoint without a config reload).
    pub fn effective_webhook_url(&self) -> Option<String> {
        let env = std::env::var(WEBHOOK_ENV).ok();
        pick_webhook(self.webhook_url.as_deref(), env.as_deref())
    }

    /// Threshold view consumed by [`AlertEvaluator`].
    pub fn thresholds(&self) -> AlertThresholds {
        AlertThresholds {
            ttft_slow_ms: self.ttft_slow_ms,
            stream_stall_gap_ms: self.stream_stall_gap_ms,
            stream_stall_critical_ms: self.stream_stall_critical_ms,
            retry_burst_warning: self.retry_burst_warning,
            retry_burst_critical: self.retry_burst_critical,
            cooldown: Duration::from_secs(self.cooldown_secs),
        }
    }
}

/// Pure selection rule (kept out of `effective_webhook_url` so it is testable
/// without touching process env).
fn pick_webhook(config: Option<&str>, env: Option<&str>) -> Option<String> {
    let non_empty = |s: &str| {
        let t = s.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    };
    env.and_then(non_empty)
        .or_else(|| config.and_then(non_empty))
}

/// Alert severity. `critical` is reserved for the escalation thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AlertLevel {
    Warning,
    Critical,
}

impl AlertLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            AlertLevel::Warning => "warning",
            AlertLevel::Critical => "critical",
        }
    }
}

/// Event kinds. Stable wire strings — dashboards/IM filters key off these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AlertKind {
    TtftSlow,
    StreamStall,
    RetryBurst,
}

impl AlertKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AlertKind::TtftSlow => "ttft_slow",
            AlertKind::StreamStall => "stream_stall",
            AlertKind::RetryBurst => "retry_burst",
        }
    }
}

/// One alert event. Field names are the handover spec §4.2 contract; the
/// webhook body wraps this in `{event, level, message, ts, fields}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertEvent {
    pub event: String,
    pub level: AlertLevel,
    /// epoch milliseconds
    pub ts: u64,
    pub request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttft_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_gap_ms: Option<f64>,
    pub retries: u32,
    pub total_latency_ms: f64,
    pub message: String,
}

impl AlertEvent {
    /// Webhook payload: `{event, level, message, ts, fields}` (handover §4.3).
    pub fn webhook_body(&self) -> serde_json::Value {
        serde_json::json!({
            "event": self.event,
            "level": self.level.as_str(),
            "message": self.message,
            "ts": self.ts,
            "fields": {
                "request_id": self.request_id,
                "model": self.model,
                "provider": self.provider,
                "ttft_ms": self.ttft_ms,
                "max_gap_ms": self.max_gap_ms,
                "retries": self.retries,
                "total_latency_ms": self.total_latency_ms,
            }
        })
    }
}

/// One request's measured facts, as delivered by the telemetry projection.
#[derive(Debug, Clone, Default)]
pub struct RequestObservation {
    pub request_id: String,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub ttft_ms: Option<f64>,
    pub max_gap_ms: Option<f64>,
    pub retries: u32,
    pub total_latency_ms: f64,
}

/// Thresholds + cooldown window. `cooldown == 0` disables merging.
#[derive(Debug, Clone)]
pub struct AlertThresholds {
    pub ttft_slow_ms: f64,
    pub stream_stall_gap_ms: f64,
    pub stream_stall_critical_ms: f64,
    pub retry_burst_warning: u32,
    pub retry_burst_critical: u32,
    pub cooldown: Duration,
}

impl Default for AlertThresholds {
    fn default() -> Self {
        AlertConfig::default().thresholds()
    }
}

#[derive(Debug, Clone, Copy)]
struct CooldownEntry {
    last: Instant,
    suppressed: u64,
}

/// Threshold evaluation + per-kind cooldown. Deliberately free of I/O so the
/// whole rule set is testable against a synthetic clock.
#[derive(Debug, Default)]
pub struct AlertEvaluator {
    last_seen: Mutex<HashMap<AlertKind, CooldownEntry>>,
}

impl AlertEvaluator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Evaluate with the wall clock (production path).
    pub fn evaluate(
        &self,
        thresholds: &AlertThresholds,
        obs: &RequestObservation,
    ) -> Vec<AlertEvent> {
        self.evaluate_at(thresholds, obs, Instant::now())
    }

    /// Evaluate against an explicit instant. Tests drive this directly so
    /// cooldown-window behaviour needs no sleeping.
    pub fn evaluate_at(
        &self,
        thresholds: &AlertThresholds,
        obs: &RequestObservation,
        now: Instant,
    ) -> Vec<AlertEvent> {
        let mut out = Vec::new();
        // Candidates are computed first, then each is passed through the
        // cooldown gate — a suppressed candidate must not consume another
        // kind's window.
        let mut candidates: Vec<(AlertKind, AlertLevel, String)> = Vec::new();

        if let Some(ttft) = obs.ttft_ms {
            if ttft > thresholds.ttft_slow_ms {
                candidates.push((
                    AlertKind::TtftSlow,
                    AlertLevel::Warning,
                    format!(
                        "首 token 延迟 {:.0}ms 超过阈值 {:.0}ms",
                        ttft, thresholds.ttft_slow_ms
                    ),
                ));
            }
        }

        if let Some(gap) = obs.max_gap_ms {
            if gap > thresholds.stream_stall_critical_ms {
                candidates.push((
                    AlertKind::StreamStall,
                    AlertLevel::Critical,
                    format!(
                        "流式静默 {:.0}ms 超过 critical 阈值 {:.0}ms",
                        gap, thresholds.stream_stall_critical_ms
                    ),
                ));
            } else if gap > thresholds.stream_stall_gap_ms {
                candidates.push((
                    AlertKind::StreamStall,
                    AlertLevel::Warning,
                    format!(
                        "流式块间 gap {:.0}ms 超过阈值 {:.0}ms",
                        gap, thresholds.stream_stall_gap_ms
                    ),
                ));
            }
        }

        if obs.retries > thresholds.retry_burst_critical {
            candidates.push((
                AlertKind::RetryBurst,
                AlertLevel::Critical,
                format!(
                    "单请求透明重试 {} 次超过 critical 阈值 {}",
                    obs.retries, thresholds.retry_burst_critical
                ),
            ));
        } else if obs.retries > thresholds.retry_burst_warning {
            candidates.push((
                AlertKind::RetryBurst,
                AlertLevel::Warning,
                format!(
                    "单请求透明重试 {} 次超过阈值 {}",
                    obs.retries, thresholds.retry_burst_warning
                ),
            ));
        }

        let mut seen = self.last_seen.lock();
        for (kind, level, base_msg) in candidates {
            if thresholds.cooldown > Duration::ZERO {
                if let Some(entry) = seen.get_mut(&kind) {
                    if now.saturating_duration_since(entry.last) < thresholds.cooldown {
                        // Inside the window: merge, never push a second time.
                        entry.suppressed += 1;
                        continue;
                    }
                }
            }
            let suppressed = seen
                .insert(
                    kind,
                    CooldownEntry {
                        last: now,
                        suppressed: 0,
                    },
                )
                .map(|old| old.suppressed)
                .unwrap_or(0);
            let message = if suppressed > 0 {
                format!("{}（冷却期内另合并 {} 次同类事件）", base_msg, suppressed)
            } else {
                base_msg
            };
            out.push(AlertEvent {
                event: kind.as_str().to_string(),
                level,
                ts: epoch_ms(),
                request_id: obs.request_id.clone(),
                model: obs.model.clone(),
                provider: obs.provider.clone(),
                ttft_ms: obs.ttft_ms,
                max_gap_ms: obs.max_gap_ms,
                retries: obs.retries,
                total_latency_ms: obs.total_latency_ms,
                message,
            });
        }
        out
    }
}

fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Delivery boundary. Implementations must not block the caller: the gateway
/// request path calls this from inside a synchronous projection.
pub trait AlertTransport: Send + Sync {
    fn deliver(&self, event: &AlertEvent);
}

/// Detached HTTP webhook delivery: one retry, failures counted, never awaited.
pub struct HttpAlertTransport {
    url: String,
    client: reqwest::Client,
    failures: AtomicU64,
    attempts: AtomicU64,
}

impl HttpAlertTransport {
    pub fn new(url: impl Into<String>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap_or_default();
        Self {
            url: url.into(),
            client,
            failures: AtomicU64::new(0),
            attempts: AtomicU64::new(0),
        }
    }

    pub fn failures(&self) -> u64 {
        self.failures.load(Ordering::Relaxed)
    }

    pub fn attempts(&self) -> u64 {
        self.attempts.load(Ordering::Relaxed)
    }
}

impl AlertTransport for HttpAlertTransport {
    fn deliver(&self, event: &AlertEvent) {
        // No runtime (unit tests / CLI paths) → log-only, same as an unset URL.
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            tracing::debug!(
                target: "ponyllm::alert",
                event = %event.event,
                "no tokio runtime; webhook push skipped (structured log already emitted)"
            );
            return;
        };
        let body = event.webhook_body();
        let url = self.url.clone();
        let client = self.client.clone();
        // Detached: the request path must never wait on an IM/webhook endpoint.
        handle.spawn(async move {
            for attempt in 1..=2u32 {
                match client.post(&url).json(&body).send().await {
                    Ok(resp) if resp.status().is_success() => return,
                    Ok(resp) => {
                        tracing::warn!(
                            target: "ponyllm::alert",
                            status = resp.status().as_u16(),
                            attempt,
                            "webhook push rejected"
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            target: "ponyllm::alert",
                            error = %e,
                            attempt,
                            "webhook push failed"
                        );
                    }
                }
            }
            // Both attempts exhausted: count and drop. Never retried later —
            // an alert storm must not become a delivery storm.
            tracing::warn!(
                target: "ponyllm::alert",
                "webhook push dropped after 1 retry"
            );
        });
        self.attempts.fetch_add(1, Ordering::Relaxed);
        if self.url.is_empty() {
            self.failures.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// `EventBus` projection: counts transparent retries per request and evaluates
/// thresholds on the terminal stream event.
pub struct AlertProjection {
    thresholds: AlertThresholds,
    evaluator: AlertEvaluator,
    transport: Option<Arc<dyn AlertTransport>>,
    attempts_by_request: Mutex<HashMap<String, u32>>,
    emitted: AtomicU64,
}

impl AlertProjection {
    pub fn new(thresholds: AlertThresholds, transport: Option<Arc<dyn AlertTransport>>) -> Self {
        Self {
            thresholds,
            evaluator: AlertEvaluator::new(),
            transport,
            attempts_by_request: Mutex::new(HashMap::new()),
            emitted: AtomicU64::new(0),
        }
    }

    /// Alerts emitted so far (observability + tests).
    pub fn emitted(&self) -> u64 {
        self.emitted.load(Ordering::Relaxed)
    }

    /// Evaluate one observation and fan out to log + transport.
    pub fn observe(&self, obs: &RequestObservation) -> Vec<AlertEvent> {
        let events = self.evaluator.evaluate(&self.thresholds, obs);
        for event in &events {
            // Structured log is the evidence chain; the push is the pager.
            // Both are required by the spec, so log unconditionally.
            tracing::warn!(
                target: "ponyllm::alert",
                event = %event.event,
                level = event.level.as_str(),
                ts = event.ts,
                request_id = %event.request_id,
                model = ?event.model,
                provider = ?event.provider,
                ttft_ms = ?event.ttft_ms,
                max_gap_ms = ?event.max_gap_ms,
                retries = event.retries,
                total_latency_ms = event.total_latency_ms,
                message = %event.message,
                "gateway performance alert"
            );
            if let Some(t) = &self.transport {
                t.deliver(event);
            }
            self.emitted.fetch_add(1, Ordering::Relaxed);
        }
        events
    }

    fn take_retries(&self, request_id: &str) -> u32 {
        self.attempts_by_request
            .lock()
            .remove(request_id)
            .unwrap_or(0)
    }
}

impl Projection for AlertProjection {
    fn apply(&self, env: &EventEnvelope) {
        match &env.event {
            GatewayEvent::UpstreamAttemptFailed { .. } => {
                *self
                    .attempts_by_request
                    .lock()
                    .entry(env.request_id.clone())
                    .or_insert(0) += 1;
            }
            GatewayEvent::StreamCompleted { flow, .. } => {
                let retries = self.take_retries(&env.request_id);
                self.observe(&RequestObservation {
                    request_id: env.request_id.clone(),
                    model: env.model.clone(),
                    provider: env.provider.clone(),
                    ttft_ms: flow.ttft_ms,
                    max_gap_ms: flow.max_gap_ms,
                    retries,
                    total_latency_ms: flow.ttlb_ms,
                });
            }
            GatewayEvent::StreamFailed {
                flow: Some(flow), ..
            } => {
                let retries = self.take_retries(&env.request_id);
                self.observe(&RequestObservation {
                    request_id: env.request_id.clone(),
                    model: env.model.clone(),
                    provider: env.provider.clone(),
                    ttft_ms: flow.ttft_ms,
                    max_gap_ms: flow.max_gap_ms,
                    retries,
                    total_latency_ms: flow.ttlb_ms,
                });
            }
            // Terminal events that carry no flow: drop the retry bookkeeping so
            // the map cannot grow without bound on failing requests.
            GatewayEvent::StreamFailed { .. }
            | GatewayEvent::StreamCancelled { .. }
            | GatewayEvent::RequestCompleted { .. }
            | GatewayEvent::RequestFailed { .. } => {
                self.attempts_by_request.lock().remove(&env.request_id);
            }
            _ => {}
        }
    }
}

/// Build the projection from config, or `None` when alerting is disabled.
pub fn build_projection(config: &AlertConfig) -> Option<AlertProjection> {
    if !config.enabled {
        return None;
    }
    let transport: Option<Arc<dyn AlertTransport>> = config
        .effective_webhook_url()
        .map(|url| Arc::new(HttpAlertTransport::new(url)) as Arc<dyn AlertTransport>);
    if transport.is_none() {
        tracing::info!(
            target: "ponyllm::alert",
            "alerts enabled without webhook URL ({} unset) → structured log only",
            WEBHOOK_ENV
        );
    }
    Some(AlertProjection::new(config.thresholds(), transport))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thresholds() -> AlertThresholds {
        AlertThresholds {
            ttft_slow_ms: 20_000.0,
            stream_stall_gap_ms: 10_000.0,
            stream_stall_critical_ms: 60_000.0,
            retry_burst_warning: 3,
            retry_burst_critical: 10,
            cooldown: Duration::from_secs(60),
        }
    }

    fn obs(ttft: Option<f64>, gap: Option<f64>, retries: u32) -> RequestObservation {
        RequestObservation {
            request_id: "req-1".into(),
            model: Some("m".into()),
            provider: Some("p".into()),
            ttft_ms: ttft,
            max_gap_ms: gap,
            retries,
            total_latency_ms: 1234.0,
        }
    }

    #[test]
    fn ttft_threshold_is_strictly_greater_than() {
        let ev = AlertEvaluator::new();
        let t = thresholds();
        // Exactly 20000 must NOT trigger (handover §5 boundary).
        assert!(ev.evaluate(&t, &obs(Some(20_000.0), None, 0)).is_empty());
        // 20.001s must trigger.
        let got = ev.evaluate(&t, &obs(Some(20_001.0), None, 0));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].event, "ttft_slow");
        assert_eq!(got[0].level, AlertLevel::Warning);
    }

    #[test]
    fn stream_stall_escalates_to_critical() {
        let ev = AlertEvaluator::new();
        let t = thresholds();
        let warn = ev.evaluate(&t, &obs(None, Some(10_001.0), 0));
        assert_eq!(warn.len(), 1);
        assert_eq!(warn[0].event, "stream_stall");
        assert_eq!(warn[0].level, AlertLevel::Warning);

        // Fresh evaluator so the cooldown does not swallow the critical case.
        let ev2 = AlertEvaluator::new();
        let crit = ev2.evaluate(&t, &obs(None, Some(60_001.0), 0));
        assert_eq!(crit.len(), 1);
        assert_eq!(crit[0].level, AlertLevel::Critical);
    }

    #[test]
    fn retry_burst_escalates_to_critical() {
        let ev = AlertEvaluator::new();
        let t = thresholds();
        // 3 is the warning threshold → not triggered (strictly greater).
        assert!(ev.evaluate(&t, &obs(None, None, 3)).is_empty());
        let warn = ev.evaluate(&t, &obs(None, None, 4));
        assert_eq!(warn.len(), 1);
        assert_eq!(warn[0].event, "retry_burst");
        assert_eq!(warn[0].level, AlertLevel::Warning);

        let ev2 = AlertEvaluator::new();
        let crit = ev2.evaluate(&t, &obs(None, None, 11));
        assert_eq!(crit.len(), 1);
        assert_eq!(crit[0].level, AlertLevel::Critical);
    }

    #[test]
    fn no_alert_below_every_threshold() {
        let ev = AlertEvaluator::new();
        let got = ev.evaluate(&thresholds(), &obs(Some(1_000.0), Some(500.0), 1));
        assert!(got.is_empty());
    }

    #[test]
    fn cooldown_merges_same_kind_and_reports_suppressed_count() {
        let ev = AlertEvaluator::new();
        let t = thresholds();
        let t0 = Instant::now();
        assert_eq!(ev.evaluate_at(&t, &obs(Some(30_000.0), None, 0), t0).len(), 1);
        // Inside the window: merged, no new push.
        assert!(ev
            .evaluate_at(&t, &obs(Some(30_000.0), None, 0), t0 + Duration::from_secs(1))
            .is_empty());
        assert!(ev
            .evaluate_at(&t, &obs(Some(30_000.0), None, 0), t0 + Duration::from_secs(2))
            .is_empty());
        // Window expired: one event carrying the merged count.
        let after = ev.evaluate_at(&t, &obs(Some(30_000.0), None, 0), t0 + Duration::from_secs(61));
        assert_eq!(after.len(), 1);
        assert!(
            after[0].message.contains("另合并 2 次"),
            "message must carry the merged count, got {}",
            after[0].message
        );
    }

    #[test]
    fn cooldown_is_per_kind() {
        let ev = AlertEvaluator::new();
        let t = thresholds();
        let t0 = Instant::now();
        assert_eq!(ev.evaluate_at(&t, &obs(Some(30_000.0), None, 0), t0).len(), 1);
        // A different kind in the same window is NOT suppressed.
        let other = ev.evaluate_at(&t, &obs(None, Some(15_000.0), 0), t0 + Duration::from_secs(1));
        assert_eq!(other.len(), 1);
        assert_eq!(other[0].event, "stream_stall");
    }

    #[test]
    fn zero_cooldown_never_suppresses() {
        let ev = AlertEvaluator::new();
        let mut t = thresholds();
        t.cooldown = Duration::ZERO;
        let t0 = Instant::now();
        assert_eq!(ev.evaluate_at(&t, &obs(Some(30_000.0), None, 0), t0).len(), 1);
        assert_eq!(
            ev.evaluate_at(&t, &obs(Some(30_000.0), None, 0), t0).len(),
            1
        );
    }

    #[test]
    fn webhook_url_precedence() {
        assert_eq!(pick_webhook(Some("http://cfg"), None), Some("http://cfg".into()));
        assert_eq!(
            pick_webhook(Some("http://cfg"), Some("http://env")),
            Some("http://env".into())
        );
        // Empty env must not mask a configured URL.
        assert_eq!(pick_webhook(Some("http://cfg"), Some("   ")), Some("http://cfg".into()));
        assert_eq!(pick_webhook(None, Some("")), None);
        assert_eq!(pick_webhook(None, None), None);
    }

    #[test]
    fn webhook_body_shape_matches_spec() {
        let ev = AlertEvaluator::new();
        let got = ev.evaluate(&thresholds(), &obs(Some(25_000.0), None, 0));
        let body = got[0].webhook_body();
        for key in ["event", "level", "message", "ts", "fields"] {
            assert!(body.get(key).is_some(), "missing key {key}");
        }
        assert_eq!(body["event"], "ttft_slow");
        assert_eq!(body["level"], "warning");
        assert_eq!(body["fields"]["request_id"], "req-1");
    }

    /// Records instead of POSTing, so projection wiring is testable offline.
    struct RecordingTransport {
        seen: Mutex<Vec<AlertEvent>>,
    }

    impl AlertTransport for RecordingTransport {
        fn deliver(&self, event: &AlertEvent) {
            self.seen.lock().push(event.clone());
        }
    }

    fn envelope(request_id: &str, event: GatewayEvent) -> EventEnvelope {
        EventEnvelope {
            seq: 0,
            request_id: request_id.to_string(),
            session_id: None,
            provider: Some("p".into()),
            model: Some("m".into()),
            endpoint: "/v1/chat/completions".into(),
            wall_ms: 0,
            elapsed_ms: 0.0,
            event,
        }
    }

    fn flow(ttft: f64, gap: f64) -> ponyllm_core::telemetry::StreamFlowSample {
        ponyllm_core::telemetry::StreamFlowSample {
            ttft_ms: Some(ttft),
            downstream_ttft_ms: Some(ttft),
            ttlb_ms: 5_000.0,
            chunks: 10,
            bytes: 100,
            max_gap_ms: Some(gap),
            stall_count: 0,
            tps: Some(1.0),
            tpot_p50_ms: None,
            tpot_p95_ms: None,
            tpot_mean_ms: None,
            prompt_tokens: 1,
            completion_tokens: 1,
            cached_tokens: 0,
        }
    }

    #[test]
    fn projection_counts_retries_then_alerts_on_completion() {
        let rec = Arc::new(RecordingTransport {
            seen: Mutex::new(Vec::new()),
        });
        let proj = AlertProjection::new(
            thresholds(),
            Some(rec.clone() as Arc<dyn AlertTransport>),
        );
        for attempt in 0..5u32 {
            proj.apply(&envelope(
                "req-9",
                GatewayEvent::UpstreamAttemptFailed {
                    key_id: "k".into(),
                    attempt,
                    status_code: Some(502),
                    kind: "upstream".into(),
                    failover: true,
                    summary: "boom".into(),
                    detail: None,
                    latency_ms: 10.0,
                    request_snippet: None,
                },
            ));
        }
        proj.apply(&envelope(
            "req-9",
            GatewayEvent::StreamCompleted {
                flow: flow(1_000.0, 100.0),
                stages: Default::default(),
                request_snippet: None,
                response_snippet: None,
            },
        ));
        let seen = rec.seen.lock();
        assert_eq!(seen.len(), 1, "only retry_burst should fire");
        assert_eq!(seen[0].event, "retry_burst");
        assert_eq!(seen[0].retries, 5);
        assert_eq!(proj.emitted(), 1);
    }

    #[test]
    fn projection_clears_retry_bookkeeping_on_failure() {
        let proj = AlertProjection::new(thresholds(), None);
        proj.apply(&envelope(
            "req-x",
            GatewayEvent::UpstreamAttemptFailed {
                key_id: "k".into(),
                attempt: 0,
                status_code: None,
                kind: "network".into(),
                failover: true,
                summary: "boom".into(),
                detail: None,
                latency_ms: 1.0,
                request_snippet: None,
            },
        ));
        assert_eq!(proj.attempts_by_request.lock().len(), 1);
        proj.apply(&envelope(
            "req-x",
            GatewayEvent::StreamFailed {
                error: "e".into(),
                flow: None,
                stages: Default::default(),
                request_snippet: None,
            },
        ));
        assert!(proj.attempts_by_request.lock().is_empty());
    }

    #[test]
    fn disabled_config_builds_no_projection() {
        assert!(build_projection(&AlertConfig::default()).is_none());
        let enabled = AlertConfig {
            enabled: true,
            webhook_url: None,
            ..Default::default()
        };
        // Enabled without URL still yields a log-only projection.
        assert!(build_projection(&enabled).is_some());
    }
}
