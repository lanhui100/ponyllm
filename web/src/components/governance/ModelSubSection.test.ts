// @vitest-environment happy-dom
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { createApp, nextTick } from 'vue';
import ModelSubSection from './ModelSubSection.vue';
import type { ModelView } from '../../types/admin';

let upstreamImpl: () => Promise<{ provider: string; source: string; models: { id: string }[] }>;

vi.mock('../../lib/adminApi', () => ({
  adminApi: {
    getUpstreamModels: (_provider: string) => ({
      send: () => upstreamImpl(),
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
  });

  it('shows sampling/pricing badges for customized models', async () => {
    const { container, app } = mountSection({ defaultExpanded: true });
    await nextTick();
    expect(container.textContent).toContain('T=0.7');
    expect(container.textContent).toContain('￥定制');
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

    // Click 峰谷模式
    const buttons = Array.from(container.querySelectorAll('button'));
    const pvBtn = buttons.find((b) => b.textContent?.trim() === '峰谷模式');
    expect(pvBtn).toBeDefined();
    pvBtn?.click();
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
});
