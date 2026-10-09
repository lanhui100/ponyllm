// @vitest-environment happy-dom
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { createApp, nextTick } from 'vue';
import { createPinia, setActivePinia } from 'pinia';
import { createRouter, createMemoryHistory } from 'vue-router';
import DashboardView from './DashboardView.vue';
import { formatChartTimestamp } from '../utils/format';

describe('DashboardView Full Feature Integration', () => {
  let container: HTMLDivElement;
  let router: ReturnType<typeof createRouter>;

  beforeEach(() => {
    setActivePinia(createPinia());
    router = createRouter({
      history: createMemoryHistory(),
      routes: [
        { path: '/dashboard', component: DashboardView },
        { path: '/recorder', component: { template: '<div>recorder</div>' } },
        { path: '/governance', component: { template: '<div>governance</div>' } },
      ],
    });
    container = document.createElement('div');
    document.body.appendChild(container);
  });

  afterEach(() => {
    document.body.removeChild(container);
    vi.restoreAllMocks();
  });

  it('renders Dashboard with gateway UptimeBars, Provider Status with 24h/7d/30d switch, and Trend charts', async () => {
    const mockHealth = { status: 'ok', version: '0.2.30' };
    const mockMetrics = {
      total_requests: 120,
      successful_requests: 118,
      failed_requests: 2,
      total_failover: 0,
      prompt_tokens: 6000,
      completion_tokens: 4000,
      total_tokens: 10000,
      stream: {
        stream_count: 80,
        avg_ttft_ms: 150,
        avg_ttlb_ms: 900,
        avg_chunks: 12,
        total_stalls: 0,
        max_gap_ms: 18,
        avg_tps: 52.0,
        total_bytes: 20000,
        total_chunks: 800,
      },
    };
    const mockStream = {
      global: mockMetrics.stream,
      providers: {
        deepseek: {
          provider: 'deepseek',
          stream_count: 50,
          avg_ttft_ms: 120.5,
          avg_tps: 55.0,
          error_count: 0,
          status: 'healthy',
          total_tokens: 6000,
          uptime_bars: {
            slots: Array.from({ length: 40 }, (_, i) => ({
              timestamp_ms: 1000 + i * 5000,
              latency_ms: 120,
              status: 'ok',
            })),
            latest_latency_ms: 120.5,
          },
        },
      },
      gateway_uptime_bars: {
        slots: Array.from({ length: 24 }, (_, i) => ({
          timestamp_ms: 1000 + i * 5000,
          latency_ms: 15,
          status: 'ok',
        })),
        latest_latency_ms: 15.0,
      },
      dropped: 0,
    };
    const mockHistory = {
      range: '24h',
      points: [
        {
          timestamp_ms: 1000,
          qps: 5.2,
          token_throughput: 240,
          total_tokens: 10000,
          prompt_tokens: 6000,
          completion_tokens: 4000,
          avg_latency_ms: 130,
          error_rate: 1.6,
          total_requests: 120,
          failed_requests: 2,
          tokens_by_provider: { deepseek: 6000 },
          tokens_by_model: { 'deepseek-chat': 6000 },
        },
      ],
      total_requests: 120,
      total_tokens: 10000,
      provider_tokens: { deepseek: 6000 },
      model_tokens: { 'deepseek-chat': 6000 },
    };

    globalThis.fetch = vi.fn().mockImplementation((url: string) => {
      if (url.includes('/health')) {
        return Promise.resolve(new Response(JSON.stringify(mockHealth), { status: 200 }));
      }
      if (url.includes('/metrics')) {
        return Promise.resolve(new Response(JSON.stringify(mockMetrics), { status: 200 }));
      }
      if (url.includes('/stream')) {
        return Promise.resolve(new Response(JSON.stringify(mockStream), { status: 200 }));
      }
      if (url.includes('/history')) {
        return Promise.resolve(new Response(JSON.stringify(mockHistory), { status: 200 }));
      }
      return Promise.reject(new Error('Unknown url'));
    });

    await router.push('/dashboard');
    const app = createApp(DashboardView);
    app.use(router);
    app.mount(container);

    // Allow fetch snapshots to complete
    await new Promise((r) => setTimeout(r, 100));
    await nextTick();

    // 1. Verify Page Title
    expect(container.textContent).toContain('系统仪表盘');

    // 2. Verify Gateway status banner renders UptimeBars (24 gateway + 40 provider)
    const allBars = container.querySelectorAll('[data-testid="uptime-bar"]');
    expect(allBars.length).toBeGreaterThanOrEqual(64);

    // 3. Verify Provider Status table with 3 Token dimensions
    expect(container.textContent).toContain('提供商状态');
    expect(container.textContent).toContain('24小时');
    expect(container.textContent).toContain('7天');
    expect(container.textContent).toContain('30天');
    expect(container.textContent).toContain('deepseek');
    expect(container.textContent).toContain('输入');
    expect(container.textContent).not.toContain('输入 Token');
    expect(container.textContent).toContain('输出');
    expect(container.textContent).not.toContain('输出 Token');
    expect(container.textContent).toContain('缓存命中');

    // 4. Verify Trend Charts presence and Token 3-dimension switch (without '按')
    expect(container.textContent).toContain('QPS 并发洪峰');
    expect(container.textContent).toContain('Token 吞吐量分布');
    expect(container.textContent).toContain('类型');
    expect(container.textContent).toContain('提供商');
    expect(container.textContent).toContain('模型');
    expect(container.textContent).not.toContain('按 Provider');
    expect(container.textContent).not.toContain('按模型');
    expect(container.textContent).not.toContain('tokens');
    expect(container.textContent).toContain('延迟与速率起伏');
    expect(container.textContent).toContain('故障率异常波动');

    app.unmount();
  });

  it('formats TrendCharts timestamp correctly across 24h, 7d, and 30d ranges', () => {
    // 2026-09-09 14:30:00 UTC
    const ts = new Date('2026-09-09T14:30:00').getTime();
    const d = new Date(ts);
    const monthDay = `${(d.getMonth() + 1).toString().padStart(2, '0')}/${d.getDate().toString().padStart(2, '0')}`;
    const hoursMinutes = `${d.getHours().toString().padStart(2, '0')}:${d.getMinutes().toString().padStart(2, '0')}`;

    expect(formatChartTimestamp(ts, '7d')).toBe(monthDay);
    expect(formatChartTimestamp(ts, '7d')).not.toContain(':');
    expect(formatChartTimestamp(ts, '30d')).toBe(monthDay);
    expect(formatChartTimestamp(ts, '24h')).toBe(hoursMinutes);
  });

  it('renders AntigravityPoolCard when antigravity keys exist', async () => {
    const mockOverview = {
      version: '0.2.30',
      config_version: 1,
      admin_write_enabled: true,
      auth_enabled: false,
    };
    const mockProviders = [{ name: 'antigravity', base_url: 'http://localhost' }];
    const mockKeys = [
      {
        id: 'anti-key-1',
        provider: 'antigravity',
        state: 'active',
        priority: 1,
        weight: 10,
      },
    ];

    globalThis.fetch = vi.fn().mockImplementation((url: string) => {
      if (url.includes('/api/admin/overview')) {
        return Promise.resolve(new Response(JSON.stringify(mockOverview), { status: 200 }));
      }
      if (url.includes('/api/admin/providers')) {
        return Promise.resolve(new Response(JSON.stringify(mockProviders), { status: 200 }));
      }
      if (url.includes('/api/admin/models')) {
        return Promise.resolve(new Response(JSON.stringify([]), { status: 200 }));
      }
      if (url.includes('/api/admin/keys')) {
        return Promise.resolve(new Response(JSON.stringify(mockKeys), { status: 200 }));
      }
      if (url.includes('/health')) {
        return Promise.resolve(new Response(JSON.stringify({ status: 'ok' }), { status: 200 }));
      }
      return Promise.resolve(new Response(JSON.stringify({}), { status: 200 }));
    });

    await router.push('/dashboard');
    const app = createApp(DashboardView);
    app.use(router);
    app.mount(container);

    await new Promise((r) => setTimeout(r, 100));
    await nextTick();

    expect(container.textContent).toContain('Antigravity 算力池');
    expect(container.textContent).toContain('1/1 账号就绪');

    // 初始化仅对"无新鲜缓存" key 补测一次（防风控），而非全量并发探测
    const testKeyCalls = (globalThis.fetch as any).mock.calls.filter((c: any[]) =>
      c[0].includes('/api/admin/keys/anti-key-1/test')
    );
    expect(testKeyCalls.length).toBe(1);

    app.unmount();
  });

  it('serializes missing quota probes with 800ms intervals when multiple keys lack fresh cache', async () => {
    vi.useFakeTimers();
    const mockOverview = {
      version: '0.2.30',
      config_version: 1,
      admin_write_enabled: true,
      auth_enabled: false,
    };
    const mockProviders = [{ name: 'antigravity', default_protocol: 'antigravity', base_url: 'http://localhost' }];
    const mockKeys = [
      { id: 'anti-key-seq-1', provider: 'antigravity', state: 'active', priority: 1, weight: 10 },
      { id: 'anti-key-seq-2', provider: 'antigravity', state: 'active', priority: 2, weight: 10 },
    ];

    const probeOrder: string[] = [];
    globalThis.fetch = vi.fn().mockImplementation((url: string) => {
      if (url.includes('/api/admin/overview')) {
        return Promise.resolve(new Response(JSON.stringify(mockOverview), { status: 200 }));
      }
      if (url.includes('/api/admin/providers')) {
        return Promise.resolve(new Response(JSON.stringify(mockProviders), { status: 200 }));
      }
      if (url.includes('/api/admin/models')) {
        return Promise.resolve(new Response(JSON.stringify([]), { status: 200 }));
      }
      if (url.includes('/api/admin/keys/anti-key-seq-1/test')) {
        probeOrder.push('anti-key-seq-1');
        return Promise.resolve(new Response(JSON.stringify({ success: true, latency_ms: 10, message: 'ok' }), { status: 200 }));
      }
      if (url.includes('/api/admin/keys/anti-key-seq-2/test')) {
        probeOrder.push('anti-key-seq-2');
        return Promise.resolve(new Response(JSON.stringify({ success: true, latency_ms: 10, message: 'ok' }), { status: 200 }));
      }
      if (url.includes('/api/admin/keys')) {
        return Promise.resolve(new Response(JSON.stringify(mockKeys), { status: 200 }));
      }
      if (url.includes('/health')) {
        return Promise.resolve(new Response(JSON.stringify({ status: 'ok' }), { status: 200 }));
      }
      return Promise.resolve(new Response(JSON.stringify({}), { status: 200 }));
    });

    await router.push('/dashboard');
    const app = createApp(DashboardView);
    app.use(router);
    app.mount(container);

    // Initial mount ticks
    await vi.advanceTimersByTimeAsync(50);
    // Key 1 should be probed first
    expect(probeOrder).toEqual(['anti-key-seq-1']);

    // Advance 800ms: Key 2 is probed serially
    await vi.advanceTimersByTimeAsync(850);
    expect(probeOrder).toEqual(['anti-key-seq-1', 'anti-key-seq-2']);

    app.unmount();
    vi.useRealTimers();
  });

  it('filters out deleted providers from ProviderMatrix when providers config changes', async () => {
    const mockOverview = {
      version: '0.2.30',
      config_version: 1,
      admin_write_enabled: true,
      auth_enabled: false,
    };
    // Only 'openai' exists in current admin config; 'deleted_provider' was deleted
    const mockProviders = [{ name: 'openai', default_protocol: 'chat', base_url: 'http://localhost' }];
    const mockStream = {
      global: { stream_count: 10, total_bytes: 100, total_chunks: 5, total_stalls: 0 },
      providers: {
        openai: { provider: 'openai', stream_count: 10, status: 'healthy' },
        deleted_provider: { provider: 'deleted_provider', stream_count: 5, status: 'degraded' },
      },
    };

    globalThis.fetch = vi.fn().mockImplementation((url: string) => {
      if (url.includes('/api/admin/overview')) {
        return Promise.resolve(new Response(JSON.stringify(mockOverview), { status: 200 }));
      }
      if (url.includes('/api/admin/providers')) {
        return Promise.resolve(new Response(JSON.stringify(mockProviders), { status: 200 }));
      }
      if (url.includes('/api/admin/keys')) {
        return Promise.resolve(new Response(JSON.stringify([]), { status: 200 }));
      }
      if (url.includes('/api/admin/models')) {
        return Promise.resolve(new Response(JSON.stringify([]), { status: 200 }));
      }
      if (url.includes('/v1/telemetry/stream')) {
        return Promise.resolve(new Response(JSON.stringify(mockStream), { status: 200 }));
      }
      if (url.includes('/health')) {
        return Promise.resolve(new Response(JSON.stringify({ status: 'ok' }), { status: 200 }));
      }
      return Promise.resolve(new Response(JSON.stringify({}), { status: 200 }));
    });

    await router.push('/dashboard');
    const app = createApp(DashboardView);
    app.use(router);
    app.mount(container);
    await nextTick();
    await new Promise((r) => setTimeout(r, 20));

    expect(container.textContent).toContain('openai');
    expect(container.textContent).not.toContain('deleted_provider');

    app.unmount();
  });
});
