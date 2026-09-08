import { ref, computed, getCurrentInstance, onMounted, onUnmounted } from 'vue';
import { onStopPolling } from '../router';
import { useSessionStore } from '../stores/session';
import type {
  ConnectivityBarSeries,
  ConnectivitySlot,
  HealthStatus,
  MetricsSummary,
  StreamTelemetrySnapshot,
  TimeseriesHistoryResponse,
} from '../types/telemetry';

export interface TelemetryPoint {
  timestamp: number;
  qps: number;
  tokenThroughput: number;
  latencyMs: number;
  errorRate: number;
}

export interface UseTelemetryOptions {
  autoStart?: boolean;
  pollingInterval?: number;
  sseEndpoint?: string;
  baseUrl?: string;
}

export const PUBLIC_GATEWAY_PROBE_URL = 'https://tokens.ponyjob.top/health';

export function useTelemetry(options: UseTelemetryOptions = {}) {
  const {
    autoStart = true,
    pollingInterval = 1500,
    sseEndpoint = '/v1/telemetry/stream',
    baseUrl = '',
  } = options;

  const health = ref<'ok' | 'down' | 'degraded' | 'unknown'>('unknown');
  const metrics = ref<MetricsSummary | null>(null);
  const stream = ref<StreamTelemetrySnapshot | null>(null);
  const history = ref<TelemetryPoint[]>([]);
  const transport = ref<'sse' | 'polling' | 'offline'>('polling');
  const isDown = ref(false);
  const selectedRange = ref<'24h' | '7d' | '30d'>('24h');
  const historyData = ref<TimeseriesHistoryResponse | null>(null);
  const gatewaySlots = ref<ConnectivitySlot[]>([]);
  const latestGatewayLatency = ref<number | undefined>(undefined);
  const lastReqCount = ref<number | null>(null);
  const lastTokenCount = ref<number | null>(null);
  const lastTickTime = ref<number>(Date.now());

  let pollTimer: ReturnType<typeof setInterval> | null = null;
  let historyTimer: ReturnType<typeof setInterval> | null = null;
  let sseSource: EventSource | null = null;
  let unregisterStop: (() => void) | null = null;

  function getFullUrl(path: string): string {
    const base = baseUrl || (typeof window !== 'undefined' ? window.location.origin : '');
    return `${base.replace(/\/$/, '')}${path.startsWith('/') ? path : `/${path}`}`;
  }

  function getAuthHeaders(): HeadersInit {
    const session = useSessionStore();
    const headers: Record<string, string> = {};
    if (session.token) {
      headers.Authorization = `Bearer ${session.token.trim()}`;
    }
    return headers;
  }

  function appendHistoryPoint(m: MetricsSummary | null) {
    const now = Date.now();
    const elapsedSec = Math.max(0.1, (now - lastTickTime.value) / 1000);
    lastTickTime.value = now;

    let qps = 0;
    let tokenRate = 0;
    let latency = 0;
    let errorRate = 0;

    if (m) {
      if (lastReqCount.value !== null) {
        const deltaReq = Math.max(0, m.total_requests - lastReqCount.value);
        qps = Number((deltaReq / elapsedSec).toFixed(1));
      }
      lastReqCount.value = m.total_requests;

      if (lastTokenCount.value !== null) {
        const deltaTokens = Math.max(0, m.total_tokens - lastTokenCount.value);
        tokenRate = Number((deltaTokens / elapsedSec).toFixed(0));
      }
      lastTokenCount.value = m.total_tokens;

      latency = m.stream?.avg_ttft_ms ?? m.stream?.avg_ttlb_ms ?? 0;
      errorRate = m.total_requests > 0
        ? Number(((m.failed_requests / m.total_requests) * 100).toFixed(1))
        : 0;
    }

    const point: TelemetryPoint = {
      timestamp: now,
      qps,
      tokenThroughput: tokenRate,
      latencyMs: latency,
      errorRate,
    };

    history.value.push(point);
    if (history.value.length > 20) {
      history.value.shift();
    }
  }

  async function fetchHistory(range?: '24h' | '7d' | '30d') {
    const targetRange = range || selectedRange.value;
    try {
      const headers = getAuthHeaders();
      const res = await fetch(getFullUrl(`/v1/telemetry/history?range=${targetRange}`), { headers });
      if (res.ok) {
        historyData.value = (await res.json()) as TimeseriesHistoryResponse;
      }
    } catch {
      // Best-effort history fetch
    }
  }

  async function setRange(r: '24h' | '7d' | '30d') {
    selectedRange.value = r;
    await fetchHistory(r);
  }

  async function probeGatewayRtt(): Promise<{ ok: boolean; latencyMs: number }> {
    const t0 = Date.now();
    const probeUrl = `${PUBLIC_GATEWAY_PROBE_URL}?_t=${t0}`;
    try {
      const res = await fetch(probeUrl, { method: 'GET', mode: 'cors', cache: 'no-store' });
      const elapsed = Math.max(1, Date.now() - t0);
      if (res.ok) {
        return { ok: true, latencyMs: elapsed };
      }
    } catch {
      // Best-effort public probe, fallback handled by caller
    }
    return { ok: false, latencyMs: 0 };
  }

  async function fetchSnapshot() {
    try {
      const headers = getAuthHeaders();
      const localT0 = Date.now();
      const [probeRes, hRes, mRes, sRes] = await Promise.allSettled([
        probeGatewayRtt(),
        fetch(getFullUrl('/health'), { headers }),
        fetch(getFullUrl('/v1/telemetry/metrics'), { headers }),
        fetch(getFullUrl('/v1/telemetry/stream'), { headers }),
      ]);
      const localRtt = Math.max(1, Date.now() - localT0);

      // Determine public vs local latency
      let rtt = localRtt;
      let isHealthy = false;

      if (probeRes.status === 'fulfilled' && probeRes.value.ok) {
        rtt = probeRes.value.latencyMs;
        isHealthy = true;
      } else if (hRes.status === 'fulfilled' && hRes.value.ok) {
        rtt = localRtt;
        isHealthy = true;
      }

      if (hRes.status === 'fulfilled' && hRes.value.ok) {
        const hData = (await hRes.value.json()) as HealthStatus;
        health.value = hData.status === 'ok' ? 'ok' : 'degraded';
        isDown.value = false;
        latestGatewayLatency.value = rtt;
        gatewaySlots.value.push({
          timestamp_ms: Date.now(),
          latency_ms: rtt,
          status: rtt < 300 ? 'ok' : rtt < 1000 ? 'degraded' : 'down',
        });
        if (gatewaySlots.value.length > 40) gatewaySlots.value.shift();
      } else if (isHealthy) {
        health.value = 'ok';
        isDown.value = false;
        latestGatewayLatency.value = rtt;
        gatewaySlots.value.push({
          timestamp_ms: Date.now(),
          latency_ms: rtt,
          status: rtt < 300 ? 'ok' : rtt < 1000 ? 'degraded' : 'down',
        });
        if (gatewaySlots.value.length > 40) gatewaySlots.value.shift();
      } else {
        health.value = 'down';
        isDown.value = true;
        transport.value = 'offline';
        gatewaySlots.value.push({
          timestamp_ms: Date.now(),
          latency_ms: undefined,
          status: 'down',
        });
        if (gatewaySlots.value.length > 40) gatewaySlots.value.shift();
        return;
      }

      if (mRes.status === 'fulfilled' && mRes.value.ok) {
        metrics.value = (await mRes.value.json()) as MetricsSummary;
      }

      if (sRes.status === 'fulfilled' && sRes.value.ok) {
        stream.value = (await sRes.value.json()) as StreamTelemetrySnapshot;
      }

      appendHistoryPoint(metrics.value);
      transport.value = 'polling';
    } catch {
      health.value = 'down';
      isDown.value = true;
      transport.value = 'offline';
      gatewaySlots.value.push({
        timestamp_ms: Date.now(),
        latency_ms: undefined,
        status: 'down',
      });
      if (gatewaySlots.value.length > 40) gatewaySlots.value.shift();
    }
  }

  function startPollingTimer() {
    stopPolling();
    pollTimer = setInterval(() => {
      if (typeof document !== 'undefined' && document.visibilityState === 'hidden') {
        // Tab in background: pause polling to save resources
        return;
      }
      void fetchSnapshot();
    }, pollingInterval);
  }

  function startHistoryTimer() {
    stopHistoryTimer();
    // Low frequency: 60s periodic history aggregation refresh
    historyTimer = setInterval(() => {
      if (typeof document !== 'undefined' && document.visibilityState === 'hidden') {
        return;
      }
      void fetchHistory();
    }, 60000);
  }

  function stopHistoryTimer() {
    if (historyTimer) {
      clearInterval(historyTimer);
      historyTimer = null;
    }
  }

  function stopPolling() {
    if (pollTimer) {
      clearInterval(pollTimer);
      pollTimer = null;
    }
  }

  async function start() {
    void fetchHistory();
    startHistoryTimer();

    if (typeof EventSource === 'undefined') {
      await fetchSnapshot();
      startPollingTimer();
      return;
    }
    try {
      const url = getFullUrl(sseEndpoint);
      sseSource = new EventSource(url);
      sseSource.onmessage = (event) => {
        try {
          const data = JSON.parse(event.data);
          if (data.metrics) {
            metrics.value = data.metrics;
            appendHistoryPoint(metrics.value);
          }
          if (data.stream) {
            stream.value = data.stream;
          }
          transport.value = 'sse';
          health.value = 'ok';
          isDown.value = false;
        } catch {
          // parse error
        }
      };
      sseSource.onerror = () => {
        if (sseSource) {
          sseSource.close();
          sseSource = null;
        }
        void fetchSnapshot().then(() => {
          startPollingTimer();
        });
      };
    } catch {
      await fetchSnapshot();
      startPollingTimer();
    }
  }

  function stop() {
    stopPolling();
    stopHistoryTimer();
    if (sseSource) {
      sseSource.close();
      sseSource = null;
    }
  }

  async function retry() {
    health.value = 'unknown';
    isDown.value = false;
    await fetchSnapshot();
    if (!isDown.value) {
      void start();
    }
  }

  if (getCurrentInstance()) {
    onMounted(() => {
      unregisterStop = onStopPolling(stop);
      if (autoStart) {
        void start();
      }
    });

    onUnmounted(() => {
      stop();
      unregisterStop?.();
    });
  }

  const gatewayUptimeBars = computed<ConnectivityBarSeries>(() => {
    if (gatewaySlots.value.length > 0) {
      return {
        slots: gatewaySlots.value,
        latest_latency_ms: latestGatewayLatency.value,
      };
    }
    if (stream.value?.gateway_uptime_bars?.slots && stream.value.gateway_uptime_bars.slots.length > 0) {
      return stream.value.gateway_uptime_bars;
    }
    return {
      slots: gatewaySlots.value,
      latest_latency_ms: latestGatewayLatency.value,
    };
  });

  return {
    health,
    metrics,
    stream,
    history,
    transport,
    isDown,
    selectedRange,
    historyData,
    gatewayUptimeBars,
    fetchHistory,
    setRange,
    start,
    stop,
    retry,
  };
}

