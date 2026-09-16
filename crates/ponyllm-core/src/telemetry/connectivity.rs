use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

pub const DEFAULT_SLOT_COUNT: usize = 40;
pub const DEFAULT_STEP_MS: u64 = 1500; // legacy compat; gateway default is 5s below
pub const GATEWAY_SLOT_COUNT: usize = 24; // 24 * 5s = 120s (最近2分钟)
pub const GATEWAY_STEP_MS: u64 = 5000;
pub const PROVIDER_SLOT_COUNT: usize = 40; // 每柱一次调用，最近40次

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectivityStatus {
    Ok,        // 网关 < 300ms 且成功；Provider TTFT < 5s 且成功
    Degraded,  // 网关 300ms..1000ms 且成功；Provider TTFT 5s..10s 且成功
    Slow,      // Provider TTFT 10s..60s 且成功
    Down,      // 网关 >= 1000ms 或失败；Provider TTFT >= 60s 或失败
    Empty,     // 无数据/初始化
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectivitySlot {
    pub timestamp_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<f64>,
    #[serde(default = "default_tps")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tps: Option<f64>,
    pub status: ConnectivityStatus,
    /// 本次调用是否成功（2xx / 流式完整结束）。
    /// 2026-09-16 引入：用于区分"慢成功"与"真失败"，使读取时可按最新阈值重算状态；
    /// 历史快照缺失该字段时按旧阈值语义回推（见 `infer_legacy_success`）。
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub success: Option<bool>,
}

fn default_tps() -> Option<f64> {
    None
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectivityBarSeries {
    pub slots: Vec<ConnectivitySlot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_latency_ms: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RingSlot {
    #[serde(default)]
    slot_start_ms: u64,
    #[serde(default)]
    latency_sum: f64,
    #[serde(default)]
    latency_count: u32,
    #[serde(default)]
    failure_count: u32,
    #[serde(default)]
    success_count: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderConnectivitySnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_latency_ms: Option<f64>,
    #[serde(default)]
    pub ring: Vec<RingSlot>,
    #[serde(default)]
    pub calls: Vec<ConnectivitySlot>,
}

#[derive(Debug)]
struct ProviderSamplerState {
    ring: Vec<RingSlot>,
    calls: VecDeque<ConnectivitySlot>,
    latest_latency_ms: Option<f64>,
}

impl ProviderSamplerState {
    fn new(ring_capacity: usize, call_capacity: usize) -> Self {
        let _ = call_capacity;
        Self {
            ring: vec![RingSlot::default(); ring_capacity.max(1)],
            calls: VecDeque::with_capacity(call_capacity.max(1)),
            latest_latency_ms: None,
        }
    }
}

#[derive(Debug)]
pub struct ConnectivitySampler {
    gateway_slot_count: usize,
    gateway_step_ms: u64,
    provider_slot_count: usize,
    providers: RwLock<HashMap<String, Arc<RwLock<ProviderSamplerState>>>>,
}

impl Default for ConnectivitySampler {
    fn default() -> Self {
        Self {
            gateway_slot_count: GATEWAY_SLOT_COUNT,
            gateway_step_ms: GATEWAY_STEP_MS,
            provider_slot_count: PROVIDER_SLOT_COUNT,
            providers: RwLock::new(HashMap::new()),
        }
    }
}

fn classify_status(latency_ms: Option<f64>, is_success: bool) -> ConnectivityStatus {
    if !is_success {
        return ConnectivityStatus::Down;
    }
    match latency_ms {
        Some(lat) if lat >= 1000.0 => ConnectivityStatus::Down,
        Some(lat) if lat >= 300.0 => ConnectivityStatus::Degraded,
        _ => ConnectivityStatus::Ok,
    }
}

fn classify_provider_status(latency_ms: Option<f64>, is_success: bool) -> ConnectivityStatus {
    if !is_success {
        return ConnectivityStatus::Down;
    }
    match latency_ms {
        Some(lat) if lat >= 60000.0 => ConnectivityStatus::Down,
        Some(lat) if lat >= 10000.0 => ConnectivityStatus::Slow,
        Some(lat) if lat >= 5000.0 => ConnectivityStatus::Degraded,
        _ => ConnectivityStatus::Ok,
    }
}

/// 历史快照（2026-09-16 之前）没有 `success` 字段时的成功性回推：
/// 旧阈值为 Ok(<3s)/Degraded(3~5s)/Down(>=5s 或失败)。
/// - 旧 Ok/Degraded：当时必为成功；
/// - 旧 Down：可能是真失败，也可能是 >=5s 的慢成功误判 —— 无法区分，
///   统一视为成功并交由新阈值重算（>=60s 仍为 Down；5~60s 纠正为 Degraded/Slow）。
/// - 旧 Empty：无数据，保持 Empty。
fn infer_legacy_success(slot: &ConnectivitySlot) -> bool {
    match slot.status {
        ConnectivityStatus::Ok | ConnectivityStatus::Degraded | ConnectivityStatus::Slow => true,
        ConnectivityStatus::Down => true,
        ConnectivityStatus::Empty => false,
    }
}

/// 读取/恢复时统一重算 Provider 状态：用 `success`（新数据）或回推（历史快照）
/// 结合当前阈值重新分类，使历史数据自动适配最新阈值方案。
fn refresh_provider_slot_status(slot: &mut ConnectivitySlot) {
    if slot.status == ConnectivityStatus::Empty {
        return;
    }
    let is_success = slot.success.unwrap_or_else(|| infer_legacy_success(slot));
    slot.status = classify_provider_status(slot.latency_ms, is_success);
}

fn normalize_latency(latency_ms: Option<f64>) -> Option<f64> {
    match latency_ms {
        Some(lat) if lat.is_finite() && lat >= 0.0 => Some((lat * 10.0).round() / 10.0),
        _ => None,
    }
}

impl ConnectivitySampler {
    pub fn new(slot_count: usize, step_ms: u64) -> Self {
        let cap = slot_count.max(1);
        Self {
            gateway_slot_count: cap,
            gateway_step_ms: step_ms.max(100),
            provider_slot_count: cap,
            providers: RwLock::new(HashMap::new()),
        }
    }

    /// List all provider names currently tracked in the sampler
    pub fn provider_names(&self) -> Vec<String> {
        self.providers.read().keys().cloned().collect()
    }

    pub fn gateway_slot_count(&self) -> usize {
        self.gateway_slot_count
    }

    pub fn gateway_step_ms(&self) -> u64 {
        self.gateway_step_ms
    }

    pub fn provider_slot_count(&self) -> usize {
        self.provider_slot_count
    }

    fn get_or_create_state(&self, provider: &str) -> Arc<RwLock<ProviderSamplerState>> {
        {
            let read = self.providers.read();
            if let Some(s) = read.get(provider) {
                return s.clone();
            }
        }
        let mut write = self.providers.write();
        write
            .entry(provider.to_string())
            .or_insert_with(|| {
                Arc::new(RwLock::new(ProviderSamplerState::new(
                    self.gateway_slot_count,
                    self.provider_slot_count,
                )))
            })
            .clone()
    }

    /// Record a single connectivity probe or request result.
    /// gateway 按时间环聚合（5s/格，最近2分钟）；provider 按调用追加（每柱一次调用）。
    pub fn record(&self, provider: &str, timestamp_ms: u64, latency_ms: Option<f64>, is_success: bool) {
        self.record_with_tps(provider, timestamp_ms, latency_ms, None, is_success);
    }

    /// Record a connectivity probe or request result with optional generation speed (tps).
    pub fn record_with_tps(
        &self,
        provider: &str,
        timestamp_ms: u64,
        latency_ms: Option<f64>,
        tps: Option<f64>,
        is_success: bool,
    ) {
        let state_arc = self.get_or_create_state(provider);
        let mut state = state_arc.write();
        let valid_latency = normalize_latency(latency_ms);
        let valid_tps = match tps {
            Some(v) if v.is_finite() && v >= 0.0 => Some((v * 10.0).round() / 10.0),
            _ => None,
        };

        if let Some(lat) = valid_latency {
            state.latest_latency_ms = Some(lat);
        }

        if provider == "gateway" {
            if state.ring.len() != self.gateway_slot_count {
                state.ring.resize(self.gateway_slot_count.max(1), RingSlot::default());
            }
            let step = self.gateway_step_ms;
            let slot_start_ms = (timestamp_ms / step) * step;
            let idx = ((slot_start_ms / step) as usize) % self.gateway_slot_count.max(1);
            if let Some(slot) = state.ring.get_mut(idx) {
                if slot.slot_start_ms != slot_start_ms {
                    *slot = RingSlot {
                        slot_start_ms,
                        ..Default::default()
                    };
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
        } else {
            let status = classify_provider_status(valid_latency, is_success);
            state.calls.push_back(ConnectivitySlot {
                timestamp_ms,
                latency_ms: valid_latency,
                tps: valid_tps,
                status,
                success: Some(is_success),
            });
            while state.calls.len() > self.provider_slot_count {
                state.calls.pop_front();
            }
        }
    }

    /// Query the series up to `now_ms`.
    /// gateway 返回时间环（24柱/5s/2分钟）；provider 返回最近调用的连续队列。
    pub fn get_series(&self, provider: &str, now_ms: u64) -> ConnectivityBarSeries {
        if provider == "gateway" {
            return self.get_gateway_series(now_ms);
        }
        self.get_provider_series(provider, now_ms)
    }

    fn get_gateway_series(&self, now_ms: u64) -> ConnectivityBarSeries {
        let state_arc = self.providers.read().get("gateway").cloned();
        let guard_opt = state_arc.as_ref().map(|arc| arc.read());
        let latest_latency_ms = guard_opt.as_ref().and_then(|g| g.latest_latency_ms);

        let total_slots = self.gateway_slot_count as u64;
        let step = self.gateway_step_ms;
        let current_slot_start = (now_ms / step) * step;
        let start_ms =
            current_slot_start.saturating_sub((total_slots.saturating_sub(1)) * step);

        let mut slots = Vec::with_capacity(self.gateway_slot_count);
        for i in 0..self.gateway_slot_count {
            let slot_start = start_ms + (i as u64 * step);
            let idx = ((slot_start / step) as usize) % self.gateway_slot_count.max(1);

            let status_and_lat = match &guard_opt {
                Some(guard) => {
                    let slot = guard.ring.get(idx).cloned().unwrap_or_default();
                    if slot.slot_start_ms == slot_start
                        && (slot.success_count > 0 || slot.failure_count > 0)
                    {
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
                        Some((classify_status(avg_lat, slot.failure_count == 0), avg_lat))
                    } else {
                        None
                    }
                }
                None => None,
            };

            match status_and_lat {
                Some((status, avg_lat)) => slots.push(ConnectivitySlot {
                    timestamp_ms: slot_start,
                    latency_ms: avg_lat,
                    tps: None,
                    status,
                    success: None,
                }),
                None => slots.push(ConnectivitySlot {
                    timestamp_ms: slot_start,
                    latency_ms: None,
                    tps: None,
                    status: ConnectivityStatus::Empty,
                    success: None,
                }),
            }
        }

        ConnectivityBarSeries {
            slots,
            latest_latency_ms,
        }
    }

    fn get_provider_series(&self, provider: &str, now_ms: u64) -> ConnectivityBarSeries {
        let state_arc = self.providers.read().get(provider).cloned();
        let (mut calls, latest) = match &state_arc {
            Some(arc) => {
                let g = arc.read();
                (g.calls.iter().cloned().collect::<Vec<_>>(), g.latest_latency_ms)
            }
            None => (Vec::new(), None),
        };
        // 读取时统一按当前阈值重算：历史快照（无论是否经过 restore）的旧状态在此自愈。
        for slot in calls.iter_mut() {
            refresh_provider_slot_status(slot);
        }

        let total = self.provider_slot_count;
        let mut out: Vec<ConnectivitySlot> = Vec::with_capacity(total);
        if calls.len() >= total {
            out.extend(calls[calls.len() - total..].iter().cloned());
        } else {
            let pad = total - calls.len();
            let anchor = calls.first().map(|c| c.timestamp_ms).unwrap_or(now_ms);
            for i in 0..pad {
                // 补齐队首空位，保持调用连续性（无中间空洞）
                let ts = anchor.saturating_sub(((pad - i) as u64) * GATEWAY_STEP_MS);
                out.push(ConnectivitySlot {
                    timestamp_ms: ts,
                    latency_ms: None,
                    tps: None,
                    status: ConnectivityStatus::Empty,
                    success: None,
                });
            }
            out.extend(calls);
        }

        ConnectivityBarSeries {
            slots: out,
            latest_latency_ms: latest,
        }
    }

    /// 导出可持久化快照（网关环 + 各provider调用队列 + 最新延迟）。
    pub fn snapshot_state(&self) -> HashMap<String, ProviderConnectivitySnapshot> {
        let mut out = HashMap::new();
        for (name, arc) in self.providers.read().iter() {
            let g = arc.read();
            out.insert(
                name.clone(),
                ProviderConnectivitySnapshot {
                    latest_latency_ms: g.latest_latency_ms,
                    ring: g.ring.clone(),
                    calls: g.calls.iter().cloned().collect(),
                },
            );
        }
        out
    }

    /// 从快照恢复（启动时调用；截断超长队列以适配当前容量）。
    pub fn restore_state(&self, snap: HashMap<String, ProviderConnectivitySnapshot>) {
        let mut write = self.providers.write();
        for (name, ps) in snap {
            let mut state =
                ProviderSamplerState::new(self.gateway_slot_count, self.provider_slot_count);
            state.latest_latency_ms = ps.latest_latency_ms;
            if name == "gateway" {
                let mut ring = ps.ring;
                ring.resize(self.gateway_slot_count.max(1), RingSlot::default());
                if ring.len() > self.gateway_slot_count {
                    ring.truncate(self.gateway_slot_count);
                }
                state.ring = ring;
            } else {
                let mut calls: VecDeque<ConnectivitySlot> = ps
                    .calls
                    .into_iter()
                    .map(|mut slot| {
                        // 兼容历史快照：启动恢复时即用当前阈值重算（读取时还会再算一次，双保险）。
                        refresh_provider_slot_status(&mut slot);
                        slot
                    })
                    .collect();
                while calls.len() > self.provider_slot_count {
                    calls.pop_front();
                }
                state.calls = calls;
            }
            write.insert(name, Arc::new(RwLock::new(state)));
        }
    }
}

impl super::event::Projection for ConnectivitySampler {
    fn apply(&self, env: &super::event::EventEnvelope) {
        let (latency, tps, is_success) = match &env.event {
            super::event::GatewayEvent::RequestCompleted { latency_ms, tps, status_code, .. } => {
                (*latency_ms, *tps, (200..300).contains(status_code))
            }
            super::event::GatewayEvent::StreamCompleted { flow, .. } => {
                // Provider 与网关连通性优先采用首字延迟（TTFT），反映上游就绪度；无 TTFT 时优雅降级至整流耗时（TTLB）
                let lat = flow.ttft_ms.unwrap_or(flow.ttlb_ms);
                (lat, flow.tps, true)
            }
            super::event::GatewayEvent::StreamFailed { flow, .. } => {
                let lat = flow
                    .as_ref()
                    .and_then(|f| f.ttft_ms)
                    .or_else(|| flow.as_ref().map(|f| f.ttlb_ms))
                    .unwrap_or(0.0);
                let tps = flow.as_ref().and_then(|f| f.tps);
                (lat, tps, false)
            }
            super::event::GatewayEvent::RequestFailed { latency_ms, .. } => {
                (*latency_ms, None, false)
            }
            _ => return,
        };

        if let Some(p) = &env.provider {
            self.record_with_tps(p, env.wall_ms, Some(latency), tps, is_success);
        }
        self.record_with_tps("gateway", env.wall_ms, Some(latency), tps, is_success);
    }
}
