// @vitest-environment happy-dom
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { createApp, nextTick } from 'vue';
import { createPinia, setActivePinia } from 'pinia';
import { createRouter, createMemoryHistory } from 'vue-router';
import DashboardView from './DashboardView.vue';

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
              timestamp_ms: 1000 + i * 1500,
              latency_ms: 120,
              status: 'ok',
            })),
            latest_latency_ms: 120.5,
          },
        },
      },
      gateway_uptime_bars: {
        slots: Array.from({ length: 40 }, (_, i) => ({
          timestamp_ms: 1000 + i * 1500,
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
    expect(container.textContent).toContain('系统可观测大盘');

    // 2. Verify Gateway status banner renders UptimeBars (40 bars)
    const allBars = container.querySelectorAll('[data-testid="uptime-bar"]');
    expect(allBars.length).toBeGreaterThanOrEqual(40);

    // 3. Verify Provider Status table
    expect(container.textContent).toContain('提供商状态');
    expect(container.textContent).toContain('24小时');
    expect(container.textContent).toContain('7天');
    expect(container.textContent).toContain('30天');
    expect(container.textContent).toContain('deepseek');
    expect(container.textContent).toContain('Token 总量');

    // 4. Verify Trend Charts presence
    expect(container.textContent).toContain('QPS 并发洪峰');
    expect(container.textContent).toContain('Token 吞吐量分布');
    expect(container.textContent).toContain('按 Provider');
    expect(container.textContent).toContain('按模型');
    expect(container.textContent).toContain('延迟与速率起伏');
    expect(container.textContent).toContain('故障率异常波动');

    app.unmount();
  });
});
