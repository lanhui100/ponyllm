// @vitest-environment happy-dom
import { describe, it, expect } from 'vitest';
import { createApp, nextTick } from 'vue';
import AggregateModelSection from './AggregateModelSection.vue';
import type { AggregateModelView, ModelView } from '../../types/admin';

const catalog: ModelView[] = [
  {
    name: 'deepseek-flash',
    provider: 'sense',
    tier: 'Standard',
    context_window: '256k',
    thinking_default: 'Off',
    thinking_max: 'High',
  },
  {
    name: 'deepseek-v4.1-flash',
    provider: 'magpie',
    tier: 'Standard',
    context_window: '256k',
    thinking_default: 'Off',
    thinking_max: 'High',
  },
];

function mountSection(
  props: Record<string, unknown> = {},
  onUpdate?: (models: AggregateModelView[]) => Promise<void>,
) {
  const container = document.createElement('div');
  document.body.appendChild(container);
  const app = createApp(AggregateModelSection, {
    adminWriteEnabled: true,
    aggregateModels: [],
    allModels: catalog,
    ...(onUpdate ? { onUpdateAggregateModels: onUpdate } : {}),
    ...props,
  });
  app.mount(container);
  return {
    container,
    app,
    cleanup() {
      app.unmount();
      document.body.removeChild(container);
    },
  };
}

async function type(input: HTMLInputElement, value: string) {
  input.value = value;
  input.dispatchEvent(new Event('input'));
  await nextTick();
}

async function choose(select: HTMLSelectElement, value: string) {
  select.value = value;
  select.dispatchEvent(new Event('change'));
  await nextTick();
}

describe('AggregateModelSection', () => {
  it('renders the empty state when no aggregate model is configured', () => {
    const { container, cleanup } = mountSection();
    expect(container.querySelector('[data-testid="aggregate-model-empty"]')).toBeTruthy();
    cleanup();
  });

  it('creates an aggregate model, appends chain entries and emits declared order', async () => {
    let emitted: AggregateModelView[] | null = null;
    const { container, cleanup } = mountSection({}, async (models) => {
      emitted = models;
    });

    (container.querySelector('[data-testid="aggregate-model-add"]') as HTMLButtonElement).click();
    await nextTick();

    const nameInput = container.querySelector(
      '[data-testid="aggregate-model-name"]',
    ) as HTMLInputElement;
    await type(nameInput, 'deepseek-free');

    // 两次「添加链项」：先 sense 主力，再改成 magpie 兜底
    (container.querySelector('[data-testid="aggregate-chain-add-0"]') as HTMLButtonElement).click();
    await nextTick();
    (container.querySelector('[data-testid="aggregate-chain-add-0"]') as HTMLButtonElement).click();
    await nextTick();

    // 链项是两级下拉：先选 provider，再选该 provider 下真实存在的模型。
    const providers = container.querySelectorAll(
      '[data-testid^="aggregate-chain-provider-"]',
    ) as NodeListOf<HTMLSelectElement>;
    expect(providers.length).toBe(2);
    await choose(providers[0], 'sense');
    await choose(providers[1], 'magpie');

    const entries = container.querySelectorAll(
      '[data-testid^="aggregate-chain-entry-"]',
    ) as NodeListOf<HTMLSelectElement>;
    expect(entries.length).toBe(2);
    await choose(entries[0], 'deepseek-flash');
    await choose(entries[1], 'deepseek-v4.1-flash');

    (container.querySelector('[data-testid="aggregate-model-save"]') as HTMLButtonElement).click();
    await nextTick();
    await nextTick();

    expect(emitted).not.toBeNull();
    expect(emitted![0].name).toBe('deepseek-free');
    expect(emitted![0].chain).toEqual(['sense/deepseek-flash', 'magpie/deepseek-v4.1-flash']);
    expect(emitted![0].cooldown_secs).toBe(90);
    // 链即授权：不再有「是否允许付费」开关，改为静态提示
    expect(
      container.querySelector('[data-testid="aggregate-model-chain-is-authorization"]'),
    ).toBeTruthy();
    cleanup();
  });

  it('keeps an unregistered chain entry selectable instead of silently dropping it', async () => {
    const { container, cleanup } = mountSection({
      aggregateModels: [{ name: 'legacy', chain: ['retired/deepseek-flash'], cooldown_secs: 90 }],
    });
    await nextTick();

    const provider = container.querySelector(
      '[data-testid="aggregate-chain-provider-0"]',
    ) as HTMLSelectElement;
    const model = container.querySelector(
      '[data-testid="aggregate-chain-entry-0"]',
    ) as HTMLSelectElement;

    // 目录里没有 retired：下拉必须把当前值保留成一个可选项，否则保存会把它清空。
    expect(provider.value).toBe('retired');
    expect([...provider.options].some((o) => o.value === 'retired')).toBe(true);
    expect(model.value).toBe('deepseek-flash');
    expect([...model.options].some((o) => o.value === 'deepseek-flash')).toBe(true);
    cleanup();
  });

  it('reorders chain entries with the up control', async () => {
    let emitted: AggregateModelView[] | null = null;
    const { container, cleanup } = mountSection(
      {
        aggregateModels: [
          {
            name: 'deepseek-free',
            chain: ['sense/deepseek-flash', 'magpie/deepseek-v4.1-flash'],
            cooldown_secs: 90,
          },
        ],
      },
      async (models) => {
        emitted = models;
      },
    );

    (container.querySelector('[data-testid="aggregate-chain-up-1"]') as HTMLButtonElement).click();
    await nextTick();
    (container.querySelector('[data-testid="aggregate-model-save"]') as HTMLButtonElement).click();
    await nextTick();
    await nextTick();

    expect(emitted![0].chain).toEqual(['magpie/deepseek-v4.1-flash', 'sense/deepseek-flash']);
    cleanup();
  });

  it('blocks saving when two aggregate models share a name', async () => {
    const { container, cleanup } = mountSection({
      aggregateModels: [
        { name: 'dup', chain: ['sense/deepseek-flash'], cooldown_secs: 90 },
        { name: 'DUP', chain: ['magpie/deepseek-v4.1-flash'], cooldown_secs: 90 },
      ],
    });
    await nextTick();

    expect(container.querySelector('[data-testid="aggregate-model-duplicate-error"]')).toBeTruthy();
    const save = container.querySelector(
      '[data-testid="aggregate-model-save"]',
    ) as HTMLButtonElement;
    expect(save.disabled).toBe(true);
    cleanup();
  });

  it('freezes every mutating control when admin write is disabled', async () => {
    const { container, cleanup } = mountSection({
      adminWriteEnabled: false,
      aggregateModels: [
        { name: 'deepseek-free', chain: ['sense/deepseek-flash'], cooldown_secs: 90 },
      ],
    });
    await nextTick();

    for (const testId of ['aggregate-model-add', 'aggregate-model-save', 'aggregate-model-remove-0']) {
      const el = container.querySelector(`[data-testid="${testId}"]`) as HTMLButtonElement;
      expect(el.disabled).toBe(true);
    }
    const nameInput = container.querySelector(
      '[data-testid="aggregate-model-name"]',
    ) as HTMLInputElement;
    expect(nameInput.disabled).toBe(true);
    cleanup();
  });
});
