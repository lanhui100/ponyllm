use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use super::metrics::StreamFlowSample;

/// Per-node stage timings carried by completion events.
/// Each field is the duration of one pipeline segment, so the deltas between
/// consecutive lifecycle events ARE the per-node costs under test.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StageTimings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub routing_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_ttfb_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_ttft_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub downstream_ttft_ms: Option<f64>,
}

/// Single-append truth events. Lifecycle events are mandatory and O(1) per
/// request; progress events are sampled; completion events carry pre-aggregated
/// flow stats (raw per-chunk timestamps never persist).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GatewayEvent {
    RouteResolved {
        provider: String,
        translated: bool,
        routing_ms: f64,
    },
    KeySelected {
        key_id: String,
        select_ms: f64,
    },
    UpstreamAttemptFailed {
        key_id: String,
        attempt: u32,
        status_code: Option<u16>,
        kind: String,
        /// Computed at emit time via `GatewayErrorKind::triggers_failover`
        /// so projections never reimplement the rule.
        failover: bool,
        summary: String,
        detail: Option<String>,
        latency_ms: f64,
        request_snippet: Option<String>,
    },
    UpstreamHeaders {
        key_id: String,
        attempt: u32,
        ttfb_ms: f64,
    },
    StreamStarted {
        request_snippet: Option<String>,
    },
    StreamProgress {
        chunks: u64,
        bytes: u64,
    },
    StreamCompleted {
        flow: StreamFlowSample,
        stages: StageTimings,
        request_snippet: Option<String>,
    },
    StreamFailed {
        error: String,
        flow: Option<StreamFlowSample>,
        stages: StageTimings,
        request_snippet: Option<String>,
    },
    /// Client disconnected after chunks flowed: counts as a failed request
    /// but never as a stream sample (not an upstream fault).
    StreamCancelled {
        chunks: u64,
        bytes: u64,
        ttlb_ms: f64,
    },
    RequestCompleted {
        status_code: u16,
        latency_ms: f64,
        prompt_tokens: u64,
        completion_tokens: u64,
        tps: Option<f64>,
        request_snippet: Option<String>,
        response_snippet: Option<String>,
    },
    RequestFailed {
        status_code: u16,
        latency_ms: f64,
        error: String,
        request_snippet: Option<String>,
    },
    /// The telemetry pipeline itself dropped events (segment channel full).
    /// Projections must surface lossy state when this is present.
    TelemetryOverflow {
        dropped: u64,
    },
}

/// Stamped envelope. `seq` orders the global log; `elapsed_ms` is monotonic
/// from the request start (stage math); `wall_ms` serves retention/ordering.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub seq: u64,
    pub request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    pub endpoint: String,
    pub wall_ms: u64,
    pub elapsed_ms: f64,
    #[serde(flatten)]
    pub event: GatewayEvent,
}

/// Caller-side context: the only clock reads on the hot path.
#[derive(Debug, Clone)]
pub struct EventCtx {
    pub request_id: String,
    pub session_id: Option<String>,
    pub endpoint: String,
    pub start: Instant,
}

impl EventCtx {
    pub fn new(
        request_id: impl Into<String>,
        endpoint: impl Into<String>,
        start: Instant,
    ) -> Self {
        Self {
            request_id: request_id.into(),
            session_id: None,
            endpoint: endpoint.into(),
            start,
        }
    }

    pub fn elapsed_ms(&self) -> f64 {
        self.start.elapsed().as_secs_f64() * 1000.0
    }
}

/// Read-side projection: derived state only, never written by business code.
pub trait Projection: Send + Sync {
    fn apply(&self, env: &EventEnvelope);
}

fn wall_ms_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Single-append event bus: the only write path for observability.
/// In-memory projections run synchronously inside `append` (deterministic,
/// lossless); the disk segment writer drains a bounded channel (lossy with an
/// explicit overflow marker, never blocking the hot path).
pub struct EventBus {
    seq: AtomicU64,
    ring: RwLock<VecDeque<EventEnvelope>>,
    ring_capacity: usize,
    projections: RwLock<Vec<Arc<dyn Projection>>>,
    seg_tx: RwLock<Option<std::sync::mpsc::SyncSender<EventEnvelope>>>,
    seg_dropped: AtomicU64,
}

impl EventBus {
    pub fn new(ring_capacity: usize) -> Self {
        Self {
            seq: AtomicU64::new(0),
            ring: RwLock::new(VecDeque::with_capacity(ring_capacity.max(1))),
            ring_capacity: ring_capacity.max(1),
            projections: RwLock::new(Vec::new()),
            seg_tx: RwLock::new(None),
            seg_dropped: AtomicU64::new(0),
        }
    }

    pub fn add_projection(&self, p: Arc<dyn Projection>) {
        self.projections.write().push(p);
    }

    /// Attach the background disk segment drain. Called once at startup.
    pub fn attach_segment_sink(&self, tx: std::sync::mpsc::SyncSender<EventEnvelope>) {
        *self.seg_tx.write() = Some(tx);
    }

    pub fn dropped_count(&self) -> u64 {
        self.seg_dropped.load(Ordering::Relaxed)
    }

    pub fn append(
        &self,
        ctx: &EventCtx,
        provider: Option<String>,
        event: GatewayEvent,
    ) -> u64 {
        self.append_at(ctx, provider, event, wall_ms_now())
    }

    /// `wall_ms` override exists for deterministic replay tests.
    pub fn append_at(
        &self,
        ctx: &EventCtx,
        provider: Option<String>,
        event: GatewayEvent,
        wall_ms: u64,
    ) -> u64 {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let env = EventEnvelope {
            seq,
            request_id: ctx.request_id.clone(),
            session_id: ctx.session_id.clone(),
            provider,
            endpoint: ctx.endpoint.clone(),
            wall_ms,
            elapsed_ms: ctx.elapsed_ms(),
            event,
        };
        {
            let mut ring = self.ring.write();
            if ring.len() >= self.ring_capacity {
                ring.pop_front();
            }
            ring.push_back(env.clone());
        }
        for p in self.projections.read().iter() {
            p.apply(&env);
        }
        if let Some(tx) = self.seg_tx.read().as_ref() {
            if tx.try_send(env).is_err() {
                let dropped = self.seg_dropped.fetch_add(1, Ordering::Relaxed) + 1;
                let marker = EventEnvelope {
                    seq: self.seq.fetch_add(1, Ordering::Relaxed),
                    request_id: String::new(),
                    session_id: None,
                    provider: None,
                    endpoint: String::new(),
                    wall_ms: wall_ms_now(),
                    elapsed_ms: 0.0,
                    event: GatewayEvent::TelemetryOverflow { dropped },
                };
                {
                    let mut ring = self.ring.write();
                    if ring.len() >= self.ring_capacity {
                        ring.pop_front();
                    }
                    ring.push_back(marker.clone());
                }
                for p in self.projections.read().iter() {
                    p.apply(&marker);
                }
            }
        }
        seq
    }

    /// Trace view: ordered lifecycle of one model call, stitched by request_id.
    pub fn trace_for(&self, request_id: &str) -> Vec<EventEnvelope> {
        let ring = self.ring.read();
        let mut out: Vec<EventEnvelope> = ring
            .iter()
            .filter(|e| e.request_id == request_id)
            .cloned()
            .collect();
        out.sort_by_key(|e| e.seq);
        out
    }

    pub fn recent(&self, n: usize) -> Vec<EventEnvelope> {
        let ring = self.ring.read();
        ring.iter().rev().take(n).cloned().collect()
    }
}

impl std::fmt::Debug for EventBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventBus")
            .field("seq", &self.seq.load(Ordering::Relaxed))
            .field("ring_len", &self.ring.read().len())
            .field("dropped", &self.dropped_count())
            .finish()
    }
}
