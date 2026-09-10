use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use parking_lot::RwLock;

use super::event::{EventEnvelope, GatewayEvent, Projection};
use super::metrics::MetricsCollector;
use crate::pool::{NodeLatencyMetrics, ProviderFlowSnapshot};

fn ms(d: f64) -> Duration {
    Duration::from_secs_f64((d / 1000.0).max(0.0))
}

/// Derives the legacy request/token counters from the event log.
/// The ONLY writer of [`MetricsCollector`]; business code emits events.
#[derive(Debug, Default)]
pub struct MetricsProjection {
    inner: Arc<MetricsCollector>,
}

impl MetricsProjection {
    pub fn new(inner: Arc<MetricsCollector>) -> Self {
        Self { inner }
    }

    pub fn collector(&self) -> &Arc<MetricsCollector> {
        &self.inner
    }
}

impl Projection for MetricsProjection {
    fn apply(&self, env: &EventEnvelope) {
        match &env.event {
            GatewayEvent::StreamCompleted { flow, .. } => {
                let prompt = flow.prompt_tokens;
                let completion = if flow.completion_tokens > 0 {
                    flow.completion_tokens
                } else {
                    flow.chunks.max(1)
                };
                let cached = flow.cached_tokens;
                self.inner.record_stream(flow);
                self.inner.record_request(
                    &env.endpoint,
                    ms(flow.ttlb_ms),
                    prompt,
                    completion,
                    cached,
                    true,
                );
            }
            GatewayEvent::StreamFailed { flow, .. } => {
                if let Some(sample) = flow {
                    let prompt = sample.prompt_tokens;
                    let completion = if sample.completion_tokens > 0 {
                        sample.completion_tokens
                    } else {
                        sample.chunks.max(1)
                    };
                    let cached = sample.cached_tokens;
                    self.inner.record_stream(sample);
                    self.inner.record_request(
                        &env.endpoint,
                        ms(sample.ttlb_ms),
                        prompt,
                        completion,
                        cached,
                        false,
                    );
                } else {
                    self.inner.record_request(&env.endpoint, ms(0.0), 0, 0, 0, false);
                }
            }
            GatewayEvent::StreamCancelled { chunks, ttlb_ms, .. } => {
                self.inner
                    .record_request(&env.endpoint, ms(*ttlb_ms), 0, *chunks, 0, false);
            }
            GatewayEvent::RequestCompleted {
                status_code,
                latency_ms,
                prompt_tokens,
                completion_tokens,
                cached_tokens,
                ..
            } => {
                self.inner.record_request(
                    &env.endpoint,
                    ms(*latency_ms),
                    *prompt_tokens,
                    *completion_tokens,
                    *cached_tokens,
                    (200..300).contains(status_code),
                );
            }
            GatewayEvent::RequestFailed { latency_ms, .. } => {
                self.inner
                    .record_request(&env.endpoint, ms(*latency_ms), 0, 0, 0, false);
            }
            GatewayEvent::UpstreamAttemptFailed { failover: true, .. } => {
                self.inner.record_failover();
            }
            GatewayEvent::UpstreamAttemptFailed { failover: false, .. } => {}
            _ => {}
        }
    }
}

/// Derives per-provider flow snapshots from the event log.
/// Owns the node metrics map; `AppState` delegates to it.
#[derive(Debug, Default)]
pub struct StreamProjection {
    nodes: RwLock<HashMap<String, Arc<NodeLatencyMetrics>>>,
}

impl StreamProjection {
    pub fn node_for(&self, provider: &str) -> Arc<NodeLatencyMetrics> {
        let read = self.nodes.read();
        if let Some(m) = read.get(provider) {
            return m.clone();
        }
        drop(read);
        let mut write = self.nodes.write();
        write
            .entry(provider.to_string())
            .or_insert_with(|| Arc::new(NodeLatencyMetrics::default()))
            .clone()
    }

    pub fn snapshot_all(&self) -> HashMap<String, ProviderFlowSnapshot> {
        self.nodes
            .read()
            .iter()
            .map(|(k, v)| (k.clone(), v.flow_snapshot()))
            .collect()
    }

    /// 导出可持久化的节点指标快照。
    pub fn snapshot_nodes(
        &self,
    ) -> HashMap<String, crate::pool::NodeLatencySnapshot> {
        self.nodes
            .read()
            .iter()
            .map(|(k, v)| (k.clone(), v.snapshot()))
            .collect()
    }

    /// 从快照恢复节点指标（仅启动时调用）。
    pub fn restore_nodes(
        &self,
        snap: HashMap<String, crate::pool::NodeLatencySnapshot>,
    ) {
        let mut write = self.nodes.write();
        for (name, ns) in snap {
            let node = write
                .entry(name)
                .or_insert_with(|| Arc::new(NodeLatencyMetrics::default()))
                .clone();
            node.restore(&ns);
        }
    }
}

impl Projection for StreamProjection {
    fn apply(&self, env: &EventEnvelope) {
        let provider = match env.provider.as_deref() {
            Some(p) => p,
            None => return,
        };
        match &env.event {
            GatewayEvent::StreamCompleted { flow, .. } => {
                let node = self.node_for(provider);
                node.update(flow.ttft_ms, flow.tps, false);
                node.record_stream_flow(flow.tpot_mean_ms, flow.max_gap_ms, flow.stall_count);
            }
            GatewayEvent::StreamFailed { flow, .. } => {
                let node = self.node_for(provider);
                match flow {
                    Some(sample) => {
                        node.update(sample.ttft_ms, None, true);
                        node.record_stream_flow(
                            sample.tpot_mean_ms,
                            sample.max_gap_ms,
                            sample.stall_count,
                        );
                    }
                    None => {
                        node.update(None, None, true);
                    }
                }
            }
            GatewayEvent::RequestCompleted { tps, status_code, .. } => {
                let is_error = !(200..300).contains(status_code);
                self.node_for(provider).update(None, *tps, is_error);
            }
            _ => {}
        }
    }
}
