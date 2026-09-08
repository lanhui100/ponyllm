use std::collections::HashMap;
use std::sync::Arc;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

pub const DEFAULT_SLOT_COUNT: usize = 40;
pub const DEFAULT_STEP_MS: u64 = 1500; // 1.5s per slot -> 40 slots = 60s (1 minute)

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectivityStatus {
    Ok,        // < 300ms 且成功
    Degraded,  // 300ms .. 1000ms 且成功
    Down,      // >= 1000ms 或失败
    Empty,     // 无数据/初始化
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectivitySlot {
    pub timestamp_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<f64>,
    pub status: ConnectivityStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectivityBarSeries {
    pub slots: Vec<ConnectivitySlot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_latency_ms: Option<f64>,
}

#[derive(Debug, Clone, Default)]
struct RingSlot {
    slot_start_ms: u64,
    latency_sum: f64,
    latency_count: u32,
    failure_count: u32,
    success_count: u32,
}

#[derive(Debug)]
struct ProviderSamplerState {
    slots: Vec<RingSlot>,
    latest_latency_ms: Option<f64>,
}

impl ProviderSamplerState {
    fn new(capacity: usize) -> Self {
        Self {
            slots: vec![RingSlot::default(); capacity],
            latest_latency_ms: None,
        }
    }
}

#[derive(Debug)]
pub struct ConnectivitySampler {
    slot_count: usize,
    step_ms: u64,
    providers: RwLock<HashMap<String, Arc<RwLock<ProviderSamplerState>>>>,
}

impl Default for ConnectivitySampler {
    fn default() -> Self {
        Self::new(DEFAULT_SLOT_COUNT, DEFAULT_STEP_MS)
    }
}

impl ConnectivitySampler {
    pub fn new(slot_count: usize, step_ms: u64) -> Self {
        Self {
            slot_count: slot_count.max(1),
            step_ms: step_ms.max(100),
            providers: RwLock::new(HashMap::new()),
        }
    }

    /// List all provider names currently tracked in the sampler
    pub fn provider_names(&self) -> Vec<String> {
        self.providers.read().keys().cloned().collect()
    }

    /// Record a single connectivity probe or request result. O(1) complexity, constant memory.
    pub fn record(&self, provider: &str, timestamp_ms: u64, latency_ms: Option<f64>, is_success: bool) {
        let state_arc = {
            let read = self.providers.read();
            if let Some(s) = read.get(provider) {
                s.clone()
            } else {
                drop(read);
                let mut write = self.providers.write();
                write
                    .entry(provider.to_string())
                    .or_insert_with(|| Arc::new(RwLock::new(ProviderSamplerState::new(self.slot_count))))
                    .clone()
            }
        };

        let mut state = state_arc.write();
        let valid_latency = match latency_ms {
            Some(lat) if lat.is_finite() && lat >= 0.0 => Some((lat * 10.0).round() / 10.0),
            _ => None,
        };

        if let Some(lat) = valid_latency {
            state.latest_latency_ms = Some(lat);
        }

        let slot_start_ms = (timestamp_ms / self.step_ms) * self.step_ms;
        let idx = ((slot_start_ms / self.step_ms) as usize) % self.slot_count;
        let slot = &mut state.slots[idx];

        if slot.slot_start_ms != slot_start_ms {
            // New time slot overwrites expired slot in ring
            slot.slot_start_ms = slot_start_ms;
            slot.latency_sum = 0.0;
            slot.latency_count = 0;
            slot.failure_count = 0;
            slot.success_count = 0;
        }

        if let Some(lat) = valid_latency {
            slot.latency_sum += lat;
            slot.latency_count += 1;
        }

        if is_success {
            slot.success_count += 1;
        } else {
            slot.failure_count += 1;
        }
    }

    /// Query the series of exactly `slot_count` bars up to `now_ms`.
    pub fn get_series(&self, provider: &str, now_ms: u64) -> ConnectivityBarSeries {
        let state_arc = self.providers.read().get(provider).cloned();
        let guard_opt = state_arc.as_ref().map(|arc| arc.read());
        let latest_latency_ms = guard_opt.as_ref().and_then(|g| g.latest_latency_ms);

        let total_slots = self.slot_count as u64;
        let current_slot_start = (now_ms / self.step_ms) * self.step_ms;
        let start_ms = current_slot_start.saturating_sub((total_slots.saturating_sub(1)) * self.step_ms);

        let mut slots = Vec::with_capacity(self.slot_count);
        for i in 0..self.slot_count {
            let slot_start = start_ms + (i as u64 * self.step_ms);
            let idx = ((slot_start / self.step_ms) as usize) % self.slot_count;

            let status_and_lat = match &guard_opt {
                Some(guard) => {
                    let slot = &guard.slots[idx];
                    if slot.slot_start_ms == slot_start && (slot.success_count > 0 || slot.failure_count > 0) {
                        let avg_lat = if slot.latency_count > 0 {
                            let avg = slot.latency_sum / slot.latency_count as f64;
                            if avg.is_finite() {
                                Some((avg * 10.0).round() / 10.0)
                            } else {
                                None
                            }
                        } else {
                            None
                        };

                        let status = if slot.failure_count > 0 {
                            ConnectivityStatus::Down
                        } else if let Some(lat) = avg_lat {
                            if lat < 300.0 {
                                ConnectivityStatus::Ok
                            } else if lat < 1000.0 {
                                ConnectivityStatus::Degraded
                            } else {
                                ConnectivityStatus::Down
                            }
                        } else {
                            ConnectivityStatus::Ok
                        };

                        Some((status, avg_lat))
                    } else {
                        None
                    }
                }
                None => None,
            };

            match status_and_lat {
                Some((status, avg_lat)) => {
                    slots.push(ConnectivitySlot {
                        timestamp_ms: slot_start,
                        latency_ms: avg_lat,
                        status,
                    });
                }
                None => {
                    slots.push(ConnectivitySlot {
                        timestamp_ms: slot_start,
                        latency_ms: None,
                        status: ConnectivityStatus::Empty,
                    });
                }
            }
        }

        ConnectivityBarSeries {
            slots,
            latest_latency_ms,
        }
    }
}

impl super::event::Projection for ConnectivitySampler {
    fn apply(&self, env: &super::event::EventEnvelope) {
        let (latency, is_success) = match &env.event {
            super::event::GatewayEvent::RequestCompleted { latency_ms, status_code, .. } => {
                (*latency_ms, (200..300).contains(status_code))
            }
            super::event::GatewayEvent::StreamCompleted { flow, .. } => {
                (flow.ttlb_ms, true)
            }
            super::event::GatewayEvent::StreamFailed { flow, .. } => {
                (flow.as_ref().map(|f| f.ttlb_ms).unwrap_or(0.0), false)
            }
            super::event::GatewayEvent::RequestFailed { latency_ms, .. } => {
                (*latency_ms, false)
            }
            _ => return,
        };

        if let Some(p) = &env.provider {
            self.record(p, env.wall_ms, Some(latency), is_success);
        }
        self.record("gateway", env.wall_ms, Some(latency), is_success);
    }
}

