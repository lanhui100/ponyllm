// @vitest-environment happy-dom
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { createApp, nextTick } from 'vue';
import ModelSubSection from './ModelSubSection.vue';
import type { ModelView } from '../../types/admin';

let upstreamImpl: () => Promise<{ provider: string; source: string; models: { id: string }[] }>;
let proxyStatusImpl: () => Promise<any>;

vi.mock('../../lib/adminApi', () => ({
  adminApi: {
    getUpstreamModels: (_provider: string) => ({
      send: () => upstreamImpl(),
    }),
    getProxyStatus: () => ({
      send: () => proxyStatusImpl(),
    }),
  },
}));

const mockModels: ModelView[] = [
  {
    name: 'gpt-4o',
    tier: 'Smart',
    context_window: '128k',
    thinking_default: 'Off',
    thinking_max: 'High',
    provider: 'openai',
    temperature: 0.7,
    input_price: 0.15,
  },
];

function mountSection(extraProps: Record<string, unknown> = {}) {
  const container = document.createElement('div');
  document.body.appendChild(container);
  const app = createApp(ModelSubSection, {
    providerName: 'openai',
    models: mockModels,
    adminWriteEnabled: true,
    ...extraProps,
  });
  app.mount(container);
  return { container, app };
}

describe('ModelSubSection model form', () => {
  beforeEach(() => {
    upstreamImpl = async () => ({ provider: 'openai', source: 'upstream', models: [] });
    proxyStatusImpl = async () => ({
      available: true,
      proxy_url: 'http://127.0.0.1:8899',
      proxy_type: 'pproxy',
      description: '本地代理',
      hint: '',
    });
  });

  it('cleans up extra badges on rows, retains tier and shows free badge when free', async () => {
    const freeModels: ModelView[] = [
      {
        ...mockModels[0],
        name: 'mimo-free',
        tier: 'Standard',
      },
      {
        ...mockModels[0],
        name: 'gpt-4o',
        tier: 'Flagship',
        input_price: 1.0,
      },
    ];
    const { container, app } = mountSection({ defaultExpanded: true, models: freeModels });
    await nextTick();
    // 验证多余徽标已被清理
    expect(container.textContent).not.toContain('T=0.7');
    expect(container.textContent).not.toContain('￥定制');
    // 验证保留 Tier（且中文正确）
    expect(container.textContent).toContain('主力');
    expect(container.textContent).toContain('旗舰');
    // 验证免费徽标仅针对免费模型出现
    const freeBadges = container.querySelectorAll('[data-testid="model-row-free"]');
    expect(freeBadges.length).toBe(1);
    expect(freeBadges[0].textContent).toContain('免费');
    app.unmount();
    document.body.removeChild(container);
  });

  it('submits sampling, pricing and display name, omits emptied fields', async () => {
    const container = document.createElement('div');
    document.body.appendChild(container);
    let created: any = null;
    const app = createApp(ModelSubSection, {
      providerName: 'openai',
      models: [],
      adminWriteEnabled: true,
      onCreate: async (payload: any) => {
        created = payload;
      },
    });
    app.mount(container);
    await nextTick();

    // 上游无名单 -> 回退手输表单
    (container.querySelector('[data-testid="add-model-btn"]') as HTMLButtonElement).click();
    await nextTick();
    await nextTick();

    const setVal = (testid: string, v: string) => {
      const el = container.querySelector(`[data-testid="${testid}"]`) as HTMLInputElement;
      el.value = v;
      el.dispatchEvent(new Event('input'));
    };
    setVal('model-name-input', 'gpt-4o-mini');
    setVal('model-display-name-input', 'GPT-4o Mini');
    // open 高级
    (container.querySelector('[data-testid="toggle-advanced-btn"]') as HTMLButtonElement).click();
    await nextTick();
    setVal('model-temperature-input', '0.7');
    setVal('model-top-p-input', '0.9');
    setVal('model-input-price-input', '0.15');
    setVal('model-cached-price-input', '');
    setVal('model-output-price-input', '0.6');
    await nextTick();

    (container.querySelector('[data-testid="submit-model-btn"]') as HTMLButtonElement).click();
    await nextTick();
    await nextTick();

    expect(created).not.toBeNull();
    expect(created.name).toBe('gpt-4o-mini');
    expect(created.display_name).toBe('GPT-4o Mini');
    expect(created.temperature).toBe(0.7);
    expect(created.top_p).toBe(0.9);
    expect(created.input_price).toBe(0.15);
    expect(created.output_price).toBe(0.6);
    expect(created.pricing_mode).toBe('uniform');
    expect('cached_price' in created).toBe(false);

    app.unmount();
    document.body.removeChild(container);
  });

  it('supports peak-valley pricing mode with custom time periods', async () => {
    const container = document.createElement('div');
    document.body.appendChild(container);
    let created: any = null;
    const app = createApp(ModelSubSection, {
      providerName: 'openai',
      models: [],
      adminWriteEnabled: true,
      onCreate: async (payload: any) => {
        created = payload;
      },
    });
    app.mount(container);
    await nextTick();

    (container.querySelector('[data-testid="add-model-btn"]') as HTMLButtonElement).click();
    await nextTick();
    await nextTick();

    const setVal = (testid: string, v: string) => {
      const el = container.querySelector(`[data-testid="${testid}"]`) as HTMLInputElement;
      el.value = v;
      el.dispatchEvent(new Event('input'));
    };
    setVal('model-name-input', 'deepseek-pv');
    (container.querySelector('[data-testid="toggle-advanced-btn"]') as HTMLButtonElement).click();
    await nextTick();

    // Click 添加峰价特别时段
    const buttons = Array.from(container.querySelectorAll('button'));
    const addPvBtn = buttons.find((b) => b.textContent?.trim().includes('添加峰价特别时段'));
    expect(addPvBtn).toBeDefined();
    addPvBtn?.click();
    await nextTick();

    (container.querySelector('[data-testid="submit-model-btn"]') as HTMLButtonElement).click();
    await nextTick();
    await nextTick();

    expect(created).not.toBeNull();
    expect(created.name).toBe('deepseek-pv');
    expect(created.pricing_mode).toBe('peak_valley');
    expect(Array.isArray(created.pricing_periods)).toBe(true);
    expect(created.pricing_periods.length).toBeGreaterThan(0);

    app.unmount();
    document.body.removeChild(container);
  });

  it('opens the upstream picker when the provider lists models', async () => {
    upstreamImpl = async () => ({
      provider: 'openai',
      source: 'upstream',
      models: [{ id: 'gpt-4o' }, { id: 'gpt-4o-mini' }],
    });
    const notices: string[] = [];
    const container = document.createElement('div');
    document.body.appendChild(container);
    const app = createApp(ModelSubSection, {
      providerName: 'openai',
      models: mockModels,
      adminWriteEnabled: true,
      onNotice: (msg: string) => notices.push(msg),
    });
    app.mount(container);
    await nextTick();

    (container.querySelector('[data-testid="add-model-btn"]') as HTMLButtonElement).click();
    await nextTick();
    await nextTick();

    expect(container.querySelector('[data-testid="upstream-model-picker"]')).not.toBeNull();
    // gpt-4o 已存在 -> 标记已添加；gpt-4o-mini 可选
    expect(container.textContent).toContain('gpt-4o-mini');
    expect(notices).toEqual([]);

    app.unmount();
    document.body.removeChild(container);
  });

  it('falls back to manual form with a notice when the provider has no list interface', async () => {
    upstreamImpl = async () => {
      throw new Error('404');
    };
    const notices: string[] = [];
    const container = document.createElement('div');
    document.body.appendChild(container);
    const app = createApp(ModelSubSection, {
      providerName: 'openai',
      models: [],
      adminWriteEnabled: true,
      onNotice: (msg: string) => notices.push(msg),
    });
    app.mount(container);
    await nextTick();

    (container.querySelector('[data-testid="add-model-btn"]') as HTMLButtonElement).click();
    await nextTick();
    await nextTick();

    expect(container.querySelector('[data-testid="upstream-model-picker"]')).toBeNull();
    expect(notices).toEqual(['该提供商未提供模型列表接口，请手动添加']);
    // 手输表单已展开
    expect(container.querySelector('[data-testid="model-name-input"]')).not.toBeNull();

    app.unmount();
    document.body.removeChild(container);
  });

  it('submits routing priority when set and shows a priority badge on rows', async () => {
    const container = document.createElement('div');
    document.body.appendChild(container);
    let created: any = null;
    const app = createApp(ModelSubSection, {
      providerName: 'openai',
      models: mockModels,
      adminWriteEnabled: true,
      defaultExpanded: true,
      onCreate: async (payload: any) => {
        created = payload;
      },
    });
    app.mount(container);
    await nextTick();

    // Existing rows without priority render no badge.
    expect(container.querySelector('[data-testid="model-row-priority"]')).toBeNull();

    (container.querySelector('[data-testid="add-model-btn"]') as HTMLButtonElement).click();
    await nextTick();
    await nextTick();
    const setVal = (testid: string, v: string) => {
      const el = container.querySelector(`[data-testid="${testid}"]`) as HTMLInputElement;
      el.value = v;
      el.dispatchEvent(new Event('input'));
    };
    setVal('model-name-input', 'gpt-6-sol');
    (container.querySelector('[data-testid="toggle-advanced-btn"]') as HTMLButtonElement).click();
    await nextTick();
    setVal('model-priority-input', '10');
    await nextTick();

    (container.querySelector('[data-testid="submit-model-btn"]') as HTMLButtonElement).click();
    await nextTick();
    await nextTick();

    expect(created).not.toBeNull();
    expect(created.priority).toBe(10);

    app.unmount();
    document.body.removeChild(container);
  });

  it('correctly normalizes model tier when editing a model (Standard -> Smart, Flagship -> Large, Light -> Fast)', async () => {
    const testModels: ModelView[] = [
      {
        ...mockModels[0],
        name: 'model-flagship',
        tier: 'Flagship',
      },
      {
        ...mockModels[0],
        name: 'model-light',
        tier: 'Light',
      },
    ];
    const { container, app } = mountSection({ defaultExpanded: true, models: testModels });
    await nextTick();

    // 找到第一个模型 (Flagship) 的编辑按钮并点击
    const editBtns = container.querySelectorAll('[data-testid="edit-model-btn"]');
    expect(editBtns.length).toBe(2);
    (editBtns[0] as HTMLButtonElement).click();
    await nextTick();

    // 检查 Flagship 映射到了 Large 按钮激活
    const flagshipBtn = container.querySelector('[data-testid="tier-btn-large"]') as HTMLButtonElement;
    expect(flagshipBtn.className).toContain('bg-slate-900');

    // 点击第二个模型 (Light) 的编辑按钮
    (editBtns[1] as HTMLButtonElement).click();
    await nextTick();

    // 检查 Light 映射到了 Fast 按钮激活
    const lightBtn = container.querySelector('[data-testid="tier-btn-fast"]') as HTMLButtonElement;
    expect(lightBtn.className).toContain('bg-slate-900');

    app.unmount();
    document.body.removeChild(container);
  });

  describe('proxy fallback resolution (red phase acceptance tests)', () => {
    it('uses gateway/prop-configured default proxy instead of hardcoded 127.0.0.1:8899 when toggled on', async () => {
      const configuredProxy = 'http://gateway-proxy.internal:8080';
      const container = document.createElement('div');
      document.body.appendChild(container);

      const app = createApp(ModelSubSection, {
        providerName: 'openai',
        models: [],
        adminWriteEnabled: true,
        defaultProxy: configuredProxy,
      });
      app.mount(container);
      await nextTick();

      // 点击添加模型
      (container.querySelector('[data-testid="add-model-btn"]') as HTMLButtonElement).click();
      await nextTick();
      await nextTick();

      // 展开高级配置
      (container.querySelector('[data-testid="toggle-advanced-btn"]') as HTMLButtonElement).click();
      await nextTick();

      // 勾选走代理
      const proxyCheckbox = container.querySelector('[data-testid="model-proxy-enabled"]') as HTMLInputElement;
      expect(proxyCheckbox).not.toBeNull();
      proxyCheckbox.checked = true;
      proxyCheckbox.dispatchEvent(new Event('change'));
      await nextTick();

      const proxyInput = container.querySelector('[data-testid="model-proxy-input"]') as HTMLInputElement;
      expect(proxyInput).not.toBeNull();
      // 断言：当开启代理开关且未手输自定义代理时，应采纳传入的有效代理，绝不能死锁在 http://127.0.0.1:8899
      expect(proxyInput.value).not.toBe('http://127.0.0.1:8899');
      expect(proxyInput.value).toBe(configuredProxy);

      app.unmount();
      document.body.removeChild(container);
    });

    it('falls back to active proxy fetched from /api/admin/proxy/status when no prop override is provided', async () => {
      const gatewayProxy = 'http://squid-egress.corp:3128';
      proxyStatusImpl = async () => ({
        available: true,
        proxy_url: gatewayProxy,
        proxy_type: 'custom',
        description: 'Gateway Egress Proxy',
        hint: 'Kubernetes egress proxy active',
      });

      const container = document.createElement('div');
      document.body.appendChild(container);

      const app = createApp(ModelSubSection, {
        providerName: 'openai',
        models: [],
        adminWriteEnabled: true,
      });
      app.mount(container);
      await nextTick();

      (container.querySelector('[data-testid="add-model-btn"]') as HTMLButtonElement).click();
      await nextTick();
      await nextTick();

      (container.querySelector('[data-testid="toggle-advanced-btn"]') as HTMLButtonElement).click();
      await nextTick();

      const proxyCheckbox = container.querySelector('[data-testid="model-proxy-enabled"]') as HTMLInputElement;
      proxyCheckbox.checked = true;
      proxyCheckbox.dispatchEvent(new Event('change'));
      await nextTick();

      const proxyInput = container.querySelector('[data-testid="model-proxy-input"]') as HTMLInputElement;
      expect(proxyInput).not.toBeNull();
      // 断言：应当优先使用动态探活到的 proxy_url，而非硬编码的 127.0.0.1:8899
      expect(proxyInput.value).toBe(gatewayProxy);

      app.unmount();
      document.body.removeChild(container);
    });
  });
});
