import { ref, getCurrentInstance, onMounted, onUnmounted } from 'vue';
import { onStopPolling } from '../router';
import { useSessionStore } from '../stores/session';
import type {
  HealthStatus,
  MetricsSummary,
  StreamTelemetrySnapshot,
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
  const lastReqCount = ref<number | null>(null);
  const lastTokenCount = ref<number | null>(null);
  const lastTickTime = ref<number>(Date.now());

  let pollTimer: ReturnType<typeof setInterval> | null = null;
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

  async function fetchSnapshot() {
    try {
      const headers = getAuthHeaders();
      const [hRes, mRes, sRes] = await Promise.allSettled([
        fetch(getFullUrl('/health'), { headers }),
        fetch(getFullUrl('/v1/telemetry/metrics'), { headers }),
        fetch(getFullUrl('/v1/telemetry/stream'), { headers }),
      ]);

      if (hRes.status === 'fulfilled' && hRes.value.ok) {
        const hData = (await hRes.value.json()) as HealthStatus;
        health.value = hData.status === 'ok' ? 'ok' : 'degraded';
        isDown.value = false;
      } else {
        health.value = 'down';
        isDown.value = true;
        transport.value = 'offline';
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

  function stopPolling() {
    if (pollTimer) {
      clearInterval(pollTimer);
      pollTimer = null;
    }
  }

  async function start() {
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

  return {
    health,
    metrics,
    stream,
    history,
    transport,
    isDown,
    start,
    stop,
    retry,
  };
}
