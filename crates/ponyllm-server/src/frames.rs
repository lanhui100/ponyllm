use std::sync::Arc;
use std::time::Duration;
use ponyllm_core::telemetry::{
    EventEnvelope, FlightFrame, FlightRecorder, GatewayEvent, Projection, StreamFlowDetail,
};

fn latency_of(env: &EventEnvelope) -> Duration {
    Duration::from_secs_f64((env.elapsed_ms / 1000.0).max(0.0))
}

/// Derives the legacy black-box frames from the event log.
/// The ONLY writer of [`FlightRecorder`]; keeps recorder endpoint shapes stable
/// while business code emits events exactly once.
pub struct FrameConverter {
    recorder: Arc<FlightRecorder>,
}

impl FrameConverter {
    pub fn new(recorder: Arc<FlightRecorder>) -> Self {
        Self { recorder }
    }
}

impl Projection for FrameConverter {
    fn apply(&self, env: &EventEnvelope) {
        let frame = match &env.event {
            GatewayEvent::UpstreamAttemptFailed {
                key_id,
                attempt,
                status_code,
                summary,
                detail,
                latency_ms,
                request_snippet,
                ..
            } => {
                let provider = env.provider.clone();
                let key = if key_id.is_empty() {
                    provider.clone().unwrap_or_default()
                } else {
                    key_id.clone()
                };
                FlightFrame {
                    request_id: env.request_id.clone(),
                    endpoint: env.endpoint.clone(),
                    provider,
                    key_id: key,
                    raw_key: None,
                    attempt: Some(*attempt),
                    status_code: *status_code,
                    latency: Duration::from_secs_f64((latency_ms / 1000.0).max(0.0)),
                    error: Some(summary.clone()),
                    request_snippet: request_snippet.clone(),
                    response_snippet: detail.clone(),
                    stream_flow: None,
                }
            }
            GatewayEvent::StreamStarted { request_snippet } => {
                let provider = env.provider.clone().unwrap_or_default();
                FlightFrame {
                    request_id: env.request_id.clone(),
                    endpoint: env.endpoint.clone(),
                    provider: Some(provider.clone()),
                    key_id: provider,
                    raw_key: None,
                    attempt: None,
                    status_code: Some(200),
                    latency: latency_of(env),
                    error: None,
                    request_snippet: request_snippet.clone(),
                    response_snippet: Some("[STREAM_STARTED]".to_string()),
                    stream_flow: None,
                }
            }
            GatewayEvent::StreamCompleted {
                flow,
                request_snippet,
                ..
            } => {
                let provider = env.provider.clone().unwrap_or_default();
                let detail = StreamFlowDetail::from(flow);
                let summary = format!(
                    "[STREAM_COMPLETED chunks={} bytes={} ttlb_ms={:.0} max_gap_ms={:.0} stalls={}]",
                    flow.chunks,
                    flow.bytes,
                    flow.ttlb_ms,
                    flow.max_gap_ms.unwrap_or(0.0),
                    flow.stall_count,
                );
                FlightFrame {
                    request_id: env.request_id.clone(),
                    endpoint: env.endpoint.clone(),
                    provider: Some(provider.clone()),
                    key_id: provider,
                    raw_key: None,
                    attempt: None,
                    status_code: Some(200),
                    latency: latency_of(env),
                    error: None,
                    request_snippet: request_snippet.clone(),
                    response_snippet: Some(summary),
                    stream_flow: Some(detail),
                }
            }
            GatewayEvent::StreamFailed {
                error,
                flow,
                request_snippet,
                ..
            } => {
                let provider = env.provider.clone().unwrap_or_default();
                FlightFrame {
                    request_id: env.request_id.clone(),
                    endpoint: env.endpoint.clone(),
                    provider: Some(provider.clone()),
                    key_id: provider,
                    raw_key: None,
                    attempt: None,
                    status_code: None,
                    latency: latency_of(env),
                    error: Some(error.clone()),
                    request_snippet: request_snippet.clone(),
                    response_snippet: None,
                    stream_flow: flow.as_ref().map(StreamFlowDetail::from),
                }
            }
            GatewayEvent::RequestCompleted {
                status_code,
                request_snippet,
                response_snippet,
                ..
            } => {
                let provider = env.provider.clone().unwrap_or_default();
                FlightFrame {
                    request_id: env.request_id.clone(),
                    endpoint: env.endpoint.clone(),
                    provider: Some(provider.clone()),
                    key_id: provider,
                    raw_key: None,
                    attempt: None,
                    status_code: Some(*status_code),
                    latency: latency_of(env),
                    error: None,
                    request_snippet: request_snippet.clone(),
                    response_snippet: response_snippet.clone(),
                    stream_flow: None,
                }
            }
            GatewayEvent::RequestFailed {
                status_code,
                error,
                request_snippet,
                ..
            } => match env.provider.clone() {
                Some(provider) => FlightFrame {
                    request_id: env.request_id.clone(),
                    endpoint: env.endpoint.clone(),
                    provider: Some(provider.clone()),
                    key_id: provider,
                    raw_key: None,
                    attempt: None,
                    status_code: Some(*status_code),
                    latency: latency_of(env),
                    error: Some(error.clone()),
                    request_snippet: request_snippet.clone(),
                    response_snippet: None,
                    stream_flow: None,
                },
                None => FlightFrame {
                    request_id: env.request_id.clone(),
                    endpoint: env.endpoint.clone(),
                    provider: None,
                    key_id: "all_providers_failed".to_string(),
                    raw_key: None,
                    attempt: None,
                    status_code: Some(*status_code),
                    latency: latency_of(env),
                    error: Some(error.clone()),
                    request_snippet: request_snippet.clone(),
                    response_snippet: None,
                    stream_flow: None,
                },
            },
            _ => return,
        };
        self.recorder.record(frame);
    }
}
