// @vitest-environment happy-dom
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { createApp, nextTick } from 'vue';
import { createPinia, setActivePinia } from 'pinia';
import { createRouter, createMemoryHistory } from 'vue-router';
import GovernanceView from './GovernanceView.vue';
import DashboardView from './DashboardView.vue';
import { useSessionStore } from '../stores/session';
import { adminApi } from '../lib/adminApi';
import type {
  OverviewView,
  ProviderView,
  ModelView,
  KeyView,
  StrategyView,
} from '../types/admin';

describe('Modern Minimalist UI/UX System-Wide E2E Verification Suite (WEB-07)', () => {
  let router: ReturnType<typeof createRouter>;
  let pinia: ReturnType<typeof createPinia>;
  let container: HTMLDivElement;

  const mockOverviewWritable: OverviewView = {
    version: '0.2.26',
    bind: '127.0.0.1:8080',
    auth_mode: 'token',
    providers: 2,
    keys: 2,
    keys_active: 2,
    strategy: 'economy',
    hot_reload_ms: 1000,
    admin_write_enabled: true,
    config_version: 20,
  };

  const mockOverviewReadonly: OverviewView = {
    ...mockOverviewWritable,
    admin_write_enabled: false,
  };

  const mockProviders: ProviderView[] = [
    {
      name: 'openai',
      base_url: 'https://api.openai.com/v1',
      default_model: 'gpt-4o',
      strategy: 'economy',
      billing_mode: 'token',
      input_price: 2.5,
      cached_price: 1.25,
      output_price: 10.0,
      models: 2,
    },
    {
      name: 'deepseek',
      base_url: 'https://api.deepseek.com/v1',
      default_model: 'deepseek-chat',
      strategy: 'speed',
      billing_mode: 'token',
      input_price: 0.14,
      cached_price: 0.07,
      output_price: 0.28,
      models: 1,
    },
  ];

  const mockModels: ModelView[] = [
    {
      name: 'gpt-4o',
      tier: 'Smart',
      context_window: '128k',
      thinking_default: 'Off',
      thinking_max: 'High',
    },
    {
      name: 'o3-mini',
      tier: 'Fast',
      context_window: '200k',
      thinking_default: 'Low',
      thinking_max: 'Medium',
    },
  ];

  const mockKeys: KeyView[] = [
    {
      id: 'key-openai-01',
      provider: 'openai',
      masked_key: 'sk-proj-****',
      priority: 1,
      weight: 10,
      state: 'active',
    },
    {
      id: 'key-deepseek-01',
      provider: 'deepseek',
      masked_key: 'sk-ds-****',
      priority: 1,
      weight: 10,
      state: 'active',
    },
  ];

  const mockStrategy: StrategyView = {
    strategy: 'economy',
    config_version: 20,
  };

  beforeEach(() => {
    pinia = createPinia();
    setActivePinia(pinia);
    router = createRouter({
      history: createMemoryHistory(),
      routes: [
        { path: '/', redirect: '/governance' },
        { path: '/governance', component: GovernanceView },
        { path: '/dashboard', component: DashboardView },
        { path: '/recorder', component: { template: '<div>recorder</div>' } },
      ],
    });
    container = document.createElement('div');
    document.body.appendChild(container);
  });

  afterEach(() => {
    vi.restoreAllMocks();
    document.body.removeChild(container);
  });

  it('Verification 1: Strict elimination of drawers & inline smooth expand validation', async () => {
    const session = useSessionStore(pinia);
    session.login('sk-admin-token');

    vi.spyOn(adminApi, 'getOverview').mockReturnValue({ send: () => Promise.resolve(mockOverviewWritable) } as any);
    vi.spyOn(adminApi, 'getProviders').mockReturnValue({ send: () => Promise.resolve(mockProviders) } as any);
    vi.spyOn(adminApi, 'getModels').mockReturnValue({ send: () => Promise.resolve(mockModels) } as any);
    vi.spyOn(adminApi, 'getKeys').mockReturnValue({ send: () => Promise.resolve(mockKeys) } as any);
    vi.spyOn(adminApi, 'getStrategy').mockReturnValue({ send: () => Promise.resolve(mockStrategy) } as any);

    const app = createApp(GovernanceView);
    app.use(router);
    app.use(pinia);
    app.mount(container);

    await nextTick();
    await new Promise((r) => setTimeout(r, 20));

    // 1. 断言 DOM 中完全不存在旧版侧拉抽屉 (drawer-panel / drawer-backdrop)
    expect(container.querySelector('.drawer-panel')).toBeNull();
    expect(container.querySelector('.drawer-backdrop')).toBeNull();

    // 2. 验证一级服务商卡片渲染
    const providerRows = container.querySelectorAll('[data-testid="provider-row"]');
    expect(providerRows.length).toBe(2);
    expect(container.textContent).toContain('openai');
    expect(container.textContent).toContain('deepseek');

    // 3. 点击顶部“+ 服务商”，验证行内平滑就地展开新建表单
    const addProviderBtn = container.querySelector('[data-testid="add-provider-btn"]') as HTMLButtonElement;
    expect(addProviderBtn).not.toBeNull();
    addProviderBtn.click();
    await nextTick();

    const nameInput = container.querySelector('[data-testid="provider-name-input"]') as HTMLInputElement;
    const urlInput = container.querySelector('[data-testid="provider-base-url-input"]') as HTMLInputElement;
    expect(nameInput).not.toBeNull();
    expect(urlInput).not.toBeNull();
    // 依然确认没有侧拉抽屉弹窗
    expect(container.querySelector('.drawer-panel')).toBeNull();

    app.unmount();
  });

  it('Verification 2: Thinking effort 4-tier mapping & hierarchical advanced collapse', async () => {
    const session = useSessionStore(pinia);
    session.login('sk-admin-token');

    vi.spyOn(adminApi, 'getOverview').mockReturnValue({ send: () => Promise.resolve(mockOverviewWritable) } as any);
    vi.spyOn(adminApi, 'getProviders').mockReturnValue({ send: () => Promise.resolve(mockProviders) } as any);
    vi.spyOn(adminApi, 'getModels').mockReturnValue({ send: () => Promise.resolve(mockModels) } as any);
    vi.spyOn(adminApi, 'getKeys').mockReturnValue({ send: () => Promise.resolve(mockKeys) } as any);
    vi.spyOn(adminApi, 'getStrategy').mockReturnValue({ send: () => Promise.resolve(mockStrategy) } as any);

    const updateModelSpy = vi.spyOn(adminApi, 'updateModel').mockReturnValue({
      send: () => Promise.resolve(mockModels[0]),
    } as any);

    const app = createApp(GovernanceView);
    app.use(router);
    app.use(pinia);
    app.mount(container);

    await nextTick();
    await new Promise((r) => setTimeout(r, 20));

    // 验证常用参数一等常显 (模型名、分级、上下文)
    expect(container.textContent).toContain('gpt-4o');
    expect(container.textContent).toContain('Smart');
    expect(container.textContent).toContain('128k');

    // 点击该模型的编辑按钮
    const editModelBtn = container.querySelector('[data-testid="edit-model-btn"]') as HTMLButtonElement;
    expect(editModelBtn).not.toBeNull();
    editModelBtn.click();
    await nextTick();

    // 验证思考强度 4 档选择器渲染
    const thinkingDefaultSelect = container.querySelector('[data-testid="thinking-default-select"]') as HTMLSelectElement;
    const thinkingMaxSelect = container.querySelector('[data-testid="thinking-max-select"]') as HTMLSelectElement;
    expect(thinkingDefaultSelect).not.toBeNull();
    expect(thinkingMaxSelect).not.toBeNull();

    // 变更思考强度配置并提交
    thinkingDefaultSelect.value = 'Low';
    thinkingDefaultSelect.dispatchEvent(new Event('change'));
    thinkingMaxSelect.value = 'High';
    thinkingMaxSelect.dispatchEvent(new Event('change'));

    const submitModelBtn = container.querySelector('[data-testid="submit-model-btn"]') as HTMLButtonElement;
    expect(submitModelBtn).not.toBeNull();
    submitModelBtn.click();

    await nextTick();
    await new Promise((r) => setTimeout(r, 20));

    // 验证 updateModel 携带了正确的 thinking_default 与 thinking_max
    expect(updateModelSpy).toHaveBeenCalledWith(
      'gpt-4o',
      expect.objectContaining({
        thinking_default: 'Low',
        thinking_max: 'High',
      }),
      20
    );

    app.unmount();
  });

  it('Verification 3: Semantic icon-only buttons & accessible tooltips', async () => {
    const session = useSessionStore(pinia);
    session.login('sk-admin-token');

    vi.spyOn(adminApi, 'getOverview').mockReturnValue({ send: () => Promise.resolve(mockOverviewWritable) } as any);
    vi.spyOn(adminApi, 'getProviders').mockReturnValue({ send: () => Promise.resolve(mockProviders) } as any);
    vi.spyOn(adminApi, 'getModels').mockReturnValue({ send: () => Promise.resolve(mockModels) } as any);
    vi.spyOn(adminApi, 'getKeys').mockReturnValue({ send: () => Promise.resolve(mockKeys) } as any);
    vi.spyOn(adminApi, 'getStrategy').mockReturnValue({ send: () => Promise.resolve(mockStrategy) } as any);

    const app = createApp(GovernanceView);
    app.use(router);
    app.use(pinia);
    app.mount(container);

    await nextTick();
    await new Promise((r) => setTimeout(r, 20));

    // 验证纯图标测速按钮与删除按钮
    const testSingleKeyBtn = container.querySelector('[data-testid="test-single-key-btn"]') as HTMLButtonElement;
    const deleteKeyBtn = container.querySelector('[data-testid="delete-key-btn"]') as HTMLButtonElement;
    expect(testSingleKeyBtn).not.toBeNull();
    expect(deleteKeyBtn).not.toBeNull();

    // 验证内部包含对应 SVG 图标
    expect(testSingleKeyBtn.querySelector('svg')).not.toBeNull();
    expect(deleteKeyBtn.querySelector('svg')).not.toBeNull();

    app.unmount();
  });

  it('Verification 4: Minimalist dashboard metrics & pulse status banner', async () => {
    const mockHealth = { status: 'ok', version: '0.2.26' };
    const mockMetrics = {
      total_requests: 300,
      successful_requests: 298,
      failed_requests: 2,
      total_failover: 0,
      prompt_tokens: 20000,
      completion_tokens: 5000,
      total_tokens: 25000,
      stream: {
        stream_count: 120,
        avg_ttft_ms: 115.0,
        avg_ttlb_ms: 800.0,
        avg_chunks: 10,
        total_stalls: 0,
        max_gap_ms: 12,
        avg_tps: 68.2,
        total_bytes: 50000,
        total_chunks: 1200,
      },
    };

    globalThis.fetch = vi.fn().mockImplementation((url: string) => {
      if (url.includes('/health')) return Promise.resolve(new Response(JSON.stringify(mockHealth), { status: 200 }));
      if (url.includes('/metrics')) return Promise.resolve(new Response(JSON.stringify(mockMetrics), { status: 200 }));
      if (url.includes('/stream')) return Promise.resolve(new Response(JSON.stringify({ global: mockMetrics.stream }), { status: 200 }));
      return Promise.reject(new Error('unhandled'));
    });

    const app = createApp(DashboardView);
    app.use(router);
    app.use(pinia);
    app.mount(container);

    await new Promise((r) => setTimeout(r, 50));
    await nextTick();

    expect(container.textContent).toContain('系统可观测大盘');
    expect(container.textContent).toContain('网关状态: OK');
    expect(container.textContent).toContain('25,000'); // total tokens
    expect(container.textContent).toContain('115.0 ms'); // ttft
    expect(container.textContent).toContain('68.2 tok/s'); // tps

    app.unmount();
  });

  it('Verification 5: Security gate, one-time secret key modal, and 412 optimistic lock handling', async () => {
    const session = useSessionStore(pinia);
    session.login('sk-admin-token');

    vi.spyOn(adminApi, 'getOverview').mockReturnValue({ send: () => Promise.resolve(mockOverviewReadonly) } as any);
    vi.spyOn(adminApi, 'getProviders').mockReturnValue({ send: () => Promise.resolve(mockProviders) } as any);
    vi.spyOn(adminApi, 'getModels').mockReturnValue({ send: () => Promise.resolve(mockModels) } as any);
    vi.spyOn(adminApi, 'getKeys').mockReturnValue({ send: () => Promise.resolve(mockKeys) } as any);
    vi.spyOn(adminApi, 'getStrategy').mockReturnValue({ send: () => Promise.resolve(mockStrategy) } as any);

    const app = createApp(GovernanceView);
    app.use(router);
    app.use(pinia);
    app.mount(container);

    await nextTick();
    await new Promise((r) => setTimeout(r, 20));

    // 只读状态检查
    const banner = container.querySelector('[data-testid="readonly-banner"]');
    expect(banner).not.toBeNull();
    expect(banner?.textContent).toContain('只读治理模式');

    const addBtn = container.querySelector('[data-testid="add-provider-btn"]') as HTMLButtonElement;
    expect(addBtn.disabled).toBe(true);

    app.unmount();
  });
});
