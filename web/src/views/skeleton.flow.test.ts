// @vitest-environment happy-dom
/**
 * 骨架占位门禁：三页在首屏数据 pending 时展示骨架、数据到达后消失；
 * 轨迹空列表仍走原空态（不被骨架吞掉）。
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { createApp, nextTick } from 'vue';
import { createPinia, setActivePinia } from 'pinia';
import { createRouter, createMemoryHistory } from 'vue-router';
import DashboardView from './DashboardView.vue';
import GovernanceView from './GovernanceView.vue';
import RecorderView from './RecorderView.vue';
import { useSessionStore } from '../stores/session';
import { adminApi } from '../lib/adminApi';

function makeRouter(component: unknown, path: string) {
  return createRouter({
    history: createMemoryHistory(),
    routes: [{ path, component: component as never }],
  });
}

async function mountAt(
  component: unknown,
  path: string,
  container: HTMLDivElement,
  pinia: ReturnType<typeof createPinia>,
) {
  const router = makeRouter(component, path);
  await router.push(path);
  const app = createApp(component as never);
  app.use(router);
  app.use(pinia);
  app.mount(container);
  return { app, router };
}

describe('Page Skeleton Placeholders', () => {
  let pinia: ReturnType<typeof createPinia>;
  let container: HTMLDivElement;

  beforeEach(() => {
    window.sessionStorage?.clear();
    pinia = createPinia();
    setActivePinia(pinia);
    container = document.createElement('div');
    document.body.appendChild(container);
  });

  afterEach(() => {
    vi.restoreAllMocks();
    document.body.removeChild(container);
  });

  it('dashboard 首屏 pending 时展示骨架，数据到达后消失', async () => {
    // 首屏：health/unknown + metrics/stream 均 null → 挂起不返回
    let resolveHealth!: (v: unknown) => void;
    const healthGate = new Promise((resolve) => {
      resolveHealth = resolve;
    });
    globalThis.fetch = vi.fn().mockImplementation((url: string) => {
      if (url.includes('/health')) return healthGate.then((d) => new Response(JSON.stringify(d), { status: 200 }));
      if (url.includes('/metrics')) return healthGate.then((d) => new Response(JSON.stringify(d), { status: 200 }));
      if (url.includes('/stream')) return healthGate.then((d) => new Response(JSON.stringify(d), { status: 200 }));
      return Promise.reject(new Error(`unhandled ${url}`));
    });

    const { app } = await mountAt(DashboardView, '/dashboard', container, pinia);
    await new Promise((r) => setTimeout(r, 30));
    await nextTick();

    expect(container.querySelector('[data-testid="dashboard-skeleton"]')).not.toBeNull();

    // 数据到达：三路全部 ok
    resolveHealth({ status: 'ok', version: '0.0.0-test' });
    globalThis.fetch = vi.fn().mockImplementation((url: string) => {
      if (url.includes('/health')) {
        return Promise.resolve(new Response(JSON.stringify({ status: 'ok' }), { status: 200 }));
      }
      if (url.includes('/metrics')) {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              total_requests: 10,
              successful_requests: 10,
              failed_requests: 0,
              total_failover: 0,
              prompt_tokens: 100,
              completion_tokens: 50,
              total_tokens: 150,
            }),
            { status: 200 },
          ),
        );
      }
      if (url.includes('/stream')) {
        return Promise.resolve(new Response(JSON.stringify({ providers: {} }), { status: 200 }));
      }
      return Promise.reject(new Error(`unhandled ${url}`));
    });
    await new Promise((r) => setTimeout(r, 80));
    await nextTick();

    expect(container.querySelector('[data-testid="dashboard-skeleton"]')).toBeNull();
    expect(container.textContent).toContain('系统仪表盘');
    app.unmount();
  });

  it('模型管理首屏 loading 且空列表时展示骨架，有数据后消失', async () => {
    const session = useSessionStore(pinia);
    session.login('sk-admin-token');

    const writable = {
      version: '0.0.0-test',
      bind: '127.0.0.1:8080',
      auth_mode: 'token',
      providers: 1,
      keys: 0,
      keys_active: 0,
      strategy: 'economy',
      hot_reload_ms: 1000,
      admin_write_enabled: true,
      config_version: 1,
    };
    const provider = {
      name: 'openai',
      base_url: 'https://api.openai.com/v1',
      default_model: 'gpt-4o',
      strategy: 'economy',
      billing_mode: 'token',
      input_price: 0,
      cached_price: 0,
      output_price: 0,
      models: 0,
    };
    let resolveGate!: (v: unknown) => void;
    const gate = new Promise((resolve) => {
      resolveGate = resolve;
    });
    vi.spyOn(adminApi, 'getOverview').mockReturnValue({ send: () => gate.then(() => writable) } as never);
    vi.spyOn(adminApi, 'getProviders').mockReturnValue({ send: () => gate.then(() => [provider]) } as never);
    vi.spyOn(adminApi, 'getModels').mockReturnValue({ send: () => gate.then(() => []) } as never);
    vi.spyOn(adminApi, 'getKeys').mockReturnValue({ send: () => gate.then(() => []) } as never);
    vi.spyOn(adminApi, 'getStrategy').mockReturnValue({
      send: () => gate.then(() => ({ strategy: 'economy', config_version: 1 })),
    } as never);

    const { app } = await mountAt(GovernanceView, '/governance', container, pinia);
    await nextTick();
    await new Promise((r) => setTimeout(r, 10));

    expect(container.querySelector('[data-testid="governance-skeleton"]')).not.toBeNull();

    resolveGate(null);
    await nextTick();
    await new Promise((r) => setTimeout(r, 30));

    expect(container.querySelector('[data-testid="governance-skeleton"]')).toBeNull();
    expect(container.textContent).toContain('openai');
    app.unmount();
  });

  it('轨迹首屏 pending 时展示骨架；空列表加载完成后走原空态', async () => {
    let resolveFrames!: (v: unknown) => void;
    const framesGate = new Promise((resolve) => {
      resolveFrames = resolve;
    });
    globalThis.fetch = vi
      .fn()
      .mockImplementation(() => framesGate.then((d) => new Response(JSON.stringify(d), { status: 200 })));

    const { app } = await mountAt(RecorderView, '/recorder', container, pinia);
    await new Promise((r) => setTimeout(r, 30));
    await nextTick();

    expect(container.querySelector('[data-testid="recorder-skeleton"]')).not.toBeNull();

    // 空列表到达：骨架消失，原空态出现
    resolveFrames([]);
    await new Promise((r) => setTimeout(r, 30));
    await nextTick();

    expect(container.querySelector('[data-testid="recorder-skeleton"]')).toBeNull();
    expect(container.textContent).toContain('暂无匹配的轨迹帧');
    app.unmount();
  });
});
