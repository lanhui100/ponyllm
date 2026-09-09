// @vitest-environment happy-dom
import { describe, it, expect } from 'vitest';
import { createApp, nextTick } from 'vue';
import ModelSubSection from './ModelSubSection.vue';
import type { ModelView } from '../../types/admin';

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

describe('ModelSubSection model name suggestions', () => {
  it('lists union names excluding current-section models, keeps free input', async () => {
    const { container, app } = mountSection({
      suggestedModelNames: ['gpt-4o', 'deepseek-chat', 'claude-opus-4-6'],
    });
    await nextTick();

    const input = container.querySelector('[data-testid="model-name-input"]') as HTMLInputElement;
    expect(input).not.toBeNull();
    // combobox wiring: input references the datalist, free text still allowed
    const listId = input.getAttribute('list');
    expect(listId).toBeTruthy();
    const options = [...container.querySelectorAll(`#${CSS.escape(listId!)} option`)].map((o) =>
      (o as HTMLOptionElement).value,
    );
    expect(options).toContain('deepseek-chat');
    expect(options).toContain('claude-opus-4-6');
    // already in this section -> excluded to avoid 409
    expect(options).not.toContain('gpt-4o');

    input.value = 'my-custom-model';
    input.dispatchEvent(new Event('input'));
    await nextTick();
    expect((container.querySelector('[data-testid="model-name-input"]') as HTMLInputElement).value).toBe(
      'my-custom-model',
    );

    app.unmount();
    document.body.removeChild(container);
  });

  it('shows sampling/pricing badges for customized models', async () => {
    const { container, app } = mountSection({ defaultExpanded: true });
    await nextTick();
    expect(container.textContent).toContain('T=0.7');
    expect(container.textContent).toContain('￥定制');
    app.unmount();
    document.body.removeChild(container);
  });

  it('submits sampling and pricing overrides, omits emptied fields', async () => {
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

    const setVal = (testid: string, v: string) => {
      const el = container.querySelector(`[data-testid="${testid}"]`) as HTMLInputElement;
      el.value = v;
      el.dispatchEvent(new Event('input'));
    };
    setVal('model-name-input', 'gpt-4o-mini');
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
    expect(created.temperature).toBe(0.7);
    expect(created.top_p).toBe(0.9);
    expect(created.input_price).toBe(0.15);
    expect(created.output_price).toBe(0.6);
    expect('cached_price' in created).toBe(false);

    app.unmount();
    document.body.removeChild(container);
  });
});
