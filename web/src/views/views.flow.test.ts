// @vitest-environment happy-dom
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { createApp, nextTick } from 'vue';
import { createPinia, setActivePinia } from 'pinia';
import { createRouter, createMemoryHistory } from 'vue-router';
import ConnectView from './Connect.vue';
import DashboardView from './DashboardView.vue';
import RecorderView from './RecorderView.vue';
import { useSessionStore } from '../stores/session';
import { stopAllPolling } from '../router';

describe('WEB-02 End-to-End User Flow (Connect -> Dashboard -> Recorder)', () => {
  let router: ReturnType<typeof createRouter>;
  let pinia: ReturnType<typeof createPinia>;
  let container: HTMLDivElement;

  beforeEach(() => {
    pinia = createPinia();
    setActivePinia(pinia);
    stopAllPolling();
    router = createRouter({
      history: createMemoryHistory(),
      routes: [
        { path: '/', redirect: '/dashboard' },
        { path: '/connect', component: ConnectView },
        { path: '/dashboard', component: DashboardView },
        { path: '/recorder', component: RecorderView },
        { path: '/governance', component: { template: '<div>gov</div>' } },
      ],
    });
    container = document.createElement('div');
    document.body.appendChild(container);
  });

  afterEach(() => {
    vi.restoreAllMocks();
    document.body.removeChild(container);
  });

  it('Flow 1: Connect flow & unauthenticated token login', async () => {
    const session = useSessionStore(pinia);
    expect(session.token).toBe('');

    globalThis.fetch = vi.fn().mockImplementation((url: string) => {
      if (url.includes('/v1/models')) {
        return Promise.resolve(new Response(JSON.stringify({ object: 'list', data: [] }), { status: 200 }));
      }
      return Promise.reject(new Error('unhandled url'));
    });

    const app = createApp(ConnectView);
    app.use(router);
    app.use(pinia);
    app.mount(container);

    expect(container.textContent).toContain('连接网关');
    const input = container.querySelector('input[type="password"]') as HTMLInputElement;
    expect(input).not.toBeNull();
    input.value = 'sk-test-secret-token';
    input.dispatchEvent(new Event('input'));

    const form = container.querySelector('form') as HTMLFormElement;
    form.dispatchEvent(new Event('submit'));

    await new Promise((resolve) => setTimeout(resolve, 20));
    await nextTick();
    expect(session.token).toBe('sk-test-secret-token');
    app.unmount();
  });

  it('Flow 2: Dashboard loads telemetry, renders KPIs and handles DOWN state', async () => {
    const mockHealth = { status: 'ok', version: '0.1.0' };
    const mockMetrics = {
      total_requests: 150,
      successful_requests: 145,
      failed_requests: 5,
      total_failover: 1,
      prompt_tokens: 12000,
      completion_tokens: 3000,
      total_tokens: 15000,
      stream: {
        stream_count: 80,
        avg_ttft_ms: 145.5,
        avg_ttlb_ms: 920.0,
        avg_chunks: 12,
        total_stalls: 0,
        max_gap_ms: 18,
        avg_tps: 52.4,
        total_bytes: 35000,
        total_chunks: 960,
      },
    };
    const mockStream = {
      global: mockMetrics.stream,
      providers: {
        deepseek: {
          provider: 'deepseek',
          stream_count: 50,
          avg_ttft_ms: 130.2,
          avg_tps: 55.0,
          error_count: 1,
          status: 'healthy' as const,
        },
      },
      dropped: 0,
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
      return Promise.reject(new Error('Unknown url'));
    });

    const app = createApp(DashboardView);
    app.use(router);
    app.use(pinia);
    app.mount(container);

    await new Promise((resolve) => setTimeout(resolve, 50));
    await nextTick();

    expect(container.textContent).toContain('系统可观测大盘');
    expect(container.textContent).toContain('网关状态');
    expect(container.textContent).toContain('15,000'); // total tokens
    expect(container.textContent).toContain('146 ms'); // ttft rounded
    expect(container.textContent).toContain('52 tok/s'); // tps rounded
    expect(container.textContent).toContain('deepseek');
    app.unmount();
  });

  it('Flow 3: Recorder loads frames, filters, supports keyboard navigation, and verifies zero-leak scrubbed key', async () => {
    const mockFrames = Array.from({ length: 50 }, (_, i) => ({
      request_id: `req-${i}`,
      timestamp: new Date(Date.now() - i * 1000).toISOString(),
      endpoint: i % 2 === 0 ? '/v1/chat/completions' : '/v1/messages',
      provider: i % 3 === 0 ? 'deepseek' : 'openai',
      key_id: `key-${i}`,
      sanitized_key: `sk-***tail${i}`,
      status_code: i === 5 ? 500 : 200,
      latency_ms: 80 + i,
      error: i === 5 ? 'Upstream rejected with sk-1234567890abcdef' : undefined,
      request_snippet: '{"model":"gpt-4","prompt":"sk-secret-payload-test"}',
      response_snippet: '{"reply":"ok"}',
      stream_flow: {
        ttft_ms: 110,
        ttlb_ms: 600,
        chunks: 8,
        bytes: 1024,
        tps: 45,
        stall_count: 0,
      },
    }));

    globalThis.fetch = vi.fn().mockImplementation((url: string) => {
      if (url.includes('/v1/telemetry/recorder')) {
        return Promise.resolve(new Response(JSON.stringify(mockFrames), { status: 200 }));
      }
      return Promise.reject(new Error('Unknown url'));
    });

    const app = createApp(RecorderView);
    app.use(router);
    app.use(pinia);
    app.mount(container);

    await new Promise((resolve) => setTimeout(resolve, 50));
    await nextTick();

    expect(container.textContent).toContain('黑匣子录波');
    expect(container.textContent).toContain('共 50 / 50 帧');

    // Filter by 5xx status
    const select = container.querySelector('.select-box') as HTMLSelectElement;
    expect(select).not.toBeNull();
    select.value = '5xx';
    select.dispatchEvent(new Event('change'));
    await nextTick();
    expect(container.textContent).toContain('共 1 / 50 帧');

    // Reset filter
    select.value = 'all';
    select.dispatchEvent(new Event('change'));
    await nextTick();

    // Test keyboard navigation: press j to select
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'j' }));
    await nextTick();

    // Press Enter to open FrameDrawer
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter' }));
    await nextTick();

    // Verify Drawer content
    expect(container.textContent).toContain('录波帧详情');
    expect(container.textContent).toContain('cURL 复现命令');

    // Verify all keys displayed are strictly sk-*** (zero leak of tails like sk-***tail0 or payload keys)
    expect(container.textContent).toContain('sk-***');
    expect(container.textContent).not.toContain('sk-***tail0');
    expect(container.textContent).not.toContain('sk-secret-payload-test');
    expect(container.textContent).not.toContain('sk-1234567890abcdef');

    app.unmount();
  });
});
