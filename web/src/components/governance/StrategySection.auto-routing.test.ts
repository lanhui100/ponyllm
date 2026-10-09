// @vitest-environment happy-dom
import { describe, it, expect, afterEach, vi } from 'vitest';
import { createApp, nextTick } from 'vue';
import StrategySection from './StrategySection.vue';

/**
 * wave-2 acceptance contract for the Auto intelligent-routing card.
 *
 * Auto routing took over global dispatch, so the global strategy picker
 * (Economy / Speed / Reliable / Balanced) is gone. What remains is a single
 * Auto card that owns:
 *   - the `Auto智能路由` heading,
 *   - an `Auto路由模型顺序` candidate list (drag + keyboard reorderable),
 *   - a checkbox dialog over the FULL model catalogue to append candidates —
 *     no text input, no search box,
 *   - the existing save / highest-priority badge / remove affordances.
 *
 * Every case below is falsifiable against the pre-change UI: the old build
 * still renders four `strategy-card` tiles, an `Auto 智能路由优先级与高可用候选`
 * heading, a `当前网关实时动态首选执行链路` block and a free-text add input.
 */

type AnyProps = Record<string, any>;

const ALL_MODELS: Array<{ provider: string; name: string }> = [
  { provider: 'antigravity', name: 'gemini-3.8-flash' },
  { provider: 'deepseek', name: 'deepseek-v4-flash' },
  { provider: 'openai', name: 'gpt-4o' },
  { provider: 'openai', name: 'gpt-4o-mini' },
];

const AUTO_MODELS = ['gemini-3.8-flash', 'deepseek-v4-flash'];

function mountSection(props: AnyProps, listeners: AnyProps = {}) {
  const container = document.createElement('div');
  document.body.appendChild(container);
  // Mount the SFC directly as the root component: `h(StrategySection, …)`
  // forces the concrete props type and rejects a spread bag.
  const app = createApp(StrategySection, { ...props, ...listeners });
  app.mount(container);
  return { app, container };
}

function q(container: HTMLElement | null | undefined, testid: string): HTMLElement | null {
  return container ? container.querySelector<HTMLElement>(`[data-testid="${testid}"]`) : null;
}

/** Buttons are queried with a narrowed element type so `.disabled` type-checks. */
function qb(container: HTMLElement | null | undefined, testid: string): HTMLButtonElement | null {
  return container
    ? container.querySelector<HTMLButtonElement>(`[data-testid="${testid}"]`)
    : null;
}

function qa(container: HTMLElement | null | undefined, selector: string): HTMLElement[] {
  return container ? Array.from(container.querySelectorAll<HTMLElement>(selector)) : [];
}

function text(container: HTMLElement): string {
  return container.textContent || '';
}

async function flush(times = 2): Promise<void> {
  for (let i = 0; i < times; i += 1) {
    await nextTick();
  }
}

/** Click that fails loudly (and legibly) when the contract element is absent. */
function click(el: HTMLElement | null, what: string): void {
  expect(el, `expected [data-testid] for ${what} to exist`).not.toBeNull();
  el!.click();
}

function need<T>(value: T | null | undefined, what: string): T {
  expect(value ?? null, `expected ${what} to exist`).not.toBeNull();
  return value as T;
}

/** HTML5 drag sequence: source row dragged onto the target row. */
function dragOnto(source: HTMLElement, target: HTMLElement): void {
  const dt = new Event('dragstart') as Event & { dataTransfer?: DataTransfer };
  const over = new Event('dragover') as Event & { dataTransfer?: DataTransfer };
  const drop = new Event('drop') as Event & { dataTransfer?: DataTransfer };
  const end = new Event('dragend');
  source.dispatchEvent(dt);
  target.dispatchEvent(over);
  target.dispatchEvent(drop);
  source.dispatchEvent(end);
}

function dispatchDrag(el: HTMLElement, type: string): void {
  el.dispatchEvent(new Event(type, { bubbles: true }));
}

describe('StrategySection — wave-2 Auto 智能路由', () => {
  afterEach(() => {
    document.body.innerHTML = '';
    vi.restoreAllMocks();
  });

  it('renames the card to Auto智能路由 and drops the global strategy picker', async () => {
    const { app, container } = mountSection({
      adminWriteEnabled: true,
      autoModels: AUTO_MODELS,
      allModels: ALL_MODELS,
    });
    await flush();

    const body = text(container);
    expect(body).toContain('Auto智能路由');
    // The retired global strategy surface must be entirely absent.
    expect(body).not.toContain('全局分流调度策略');
    expect(body).not.toContain('Economy 经济优先');
    expect(body).not.toContain('Speed 速度优先');
    expect(body).not.toContain('Reliable 稳定优先');
    expect(body).not.toContain('Balanced 综合均衡');
    expect(qa(container, '[data-testid="strategy-card"]')).toHaveLength(0);
    // The retired live-resolution mirror block is gone as well.
    expect(body).not.toContain('当前网关实时动态首选执行链路');
    app.unmount();
  });

  it('relabels the candidate list to Auto路由模型顺序', async () => {
    const { app, container } = mountSection({
      adminWriteEnabled: true,
      autoModels: AUTO_MODELS,
      allModels: ALL_MODELS,
    });
    await flush();

    expect(text(container)).toContain('Auto路由模型顺序');
    expect(text(container)).not.toContain('自定义优先配置序列表');
    // Regression red line: the candidates themselves still render in order.
    const rows = qa(container, '[data-testid="auto-model-row"]');
    expect(rows).toHaveLength(2);
    expect(rows[0].textContent).toContain('gemini-3.8-flash');
    expect(rows[1].textContent).toContain('deepseek-v4-flash');
    // Highest-priority badge survives on the first row only.
    expect(qa(container, '[data-testid="auto-model-top-badge"]')).toHaveLength(1);
    app.unmount();
  });

  it('replaces the free-text add input with a checkbox dialog over all models', async () => {
    const { app, container } = mountSection({
      adminWriteEnabled: true,
      autoModels: AUTO_MODELS,
      allModels: ALL_MODELS,
    });
    await flush();

    // Before opening: no input, no dialog.
    expect(container.querySelectorAll('input[type="text"]')).toHaveLength(0);
    expect(q(container, 'auto-model-picker')).toBeNull();
    expect(text(container)).not.toContain('输入主力模型名称');

    click(qb(container, 'auto-model-add-open'), 'auto-model-add-open');
    await flush();

    const dialog = q(container, 'auto-model-picker');
    expect(dialog).not.toBeNull();
    // No text input and no search box inside the dialog.
    expect(dialog!.querySelectorAll('input')).toHaveLength(ALL_MODELS.length);
    expect(qa(dialog!, 'input').every((i) => i.getAttribute('type') === 'checkbox')).toBe(true);
    expect(dialog!.querySelectorAll('input[type="search"], input[type="text"]')).toHaveLength(0);

    // Every catalogue model is offered, including the ones already configured.
    const optionIds = qa(dialog!, '[data-testid="auto-model-option"]').map((el) =>
      el.getAttribute('data-model-id'),
    );
    expect(optionIds).toEqual([
      'antigravity/gemini-3.8-flash',
      'deepseek/deepseek-v4-flash',
      'openai/gpt-4o',
      'openai/gpt-4o-mini',
    ]);
    app.unmount();
  });

  it('appends checked candidates in check order, skipping duplicates, and never auto-saves', async () => {
    const onUpdateAutoModels = vi.fn();
    const { app, container } = mountSection(
      { adminWriteEnabled: true, autoModels: AUTO_MODELS, allModels: ALL_MODELS },
      { onUpdateAutoModels },
    );
    await flush();

    click(qb(container, 'auto-model-add-open'), 'auto-model-add-open');
    await flush();

    const dialog = need(q(container, 'auto-model-picker'), 'auto-model-picker');
    const boxes = qa(dialog, '[data-testid="auto-model-option"] input');
    // Check gpt-4o-mini FIRST, then gpt-4o, then an already-present model.
    (boxes[3] as HTMLInputElement).click();
    await flush();
    (boxes[2] as HTMLInputElement).click();
    await flush();
    (boxes[0] as HTMLInputElement).click();
    await flush();

    click(qb(container, 'auto-model-picker-confirm'), 'auto-model-picker-confirm');
    await flush();

    const rows = qa(container, '[data-testid="auto-model-row"]');
    expect(rows.map((r) => r.getAttribute('data-model'))).toEqual([
      'gemini-3.8-flash',
      'deepseek-v4-flash',
      'gpt-4o-mini',
      'gpt-4o',
    ]);
    // Check order, not catalogue order — and the save is still an explicit act.
    expect(onUpdateAutoModels).not.toHaveBeenCalled();
    app.unmount();
  });

  it('cancel discards the pending selection and resets checkboxes on reopen', async () => {
    const onUpdateAutoModels = vi.fn();
    const { app, container } = mountSection(
      { adminWriteEnabled: true, autoModels: AUTO_MODELS, allModels: ALL_MODELS },
      { onUpdateAutoModels },
    );
    await flush();

    click(qb(container, 'auto-model-add-open'), 'auto-model-add-open');
    await flush();
    const firstOpen = qa(need(q(container, 'auto-model-picker'), 'auto-model-picker'), '[data-testid="auto-model-option"] input');
    (firstOpen[2] as HTMLInputElement).click();
    await flush();
    click(qb(container, 'auto-model-picker-cancel'), 'auto-model-picker-cancel');
    await flush();

    expect(q(container, 'auto-model-picker')).toBeNull();
    expect(qa(container, '[data-testid="auto-model-row"]')).toHaveLength(2);
    expect(onUpdateAutoModels).not.toHaveBeenCalled();

    // Reopening starts from a clean slate — no stale check marks.
    click(qb(container, 'auto-model-add-open'), 'auto-model-add-open');
    await flush();
    const secondOpen = qa(need(q(container, 'auto-model-picker'), 'auto-model-picker'), '[data-testid="auto-model-option"] input');
    expect(secondOpen.every((i) => (i as HTMLInputElement).checked === false)).toBe(true);
    app.unmount();
  });

  it('drag-reorders the candidate list and the save event carries the new order', async () => {
    const onUpdateAutoModels = vi.fn();
    const { app, container } = mountSection(
      { adminWriteEnabled: true, autoModels: AUTO_MODELS, allModels: ALL_MODELS },
      { onUpdateAutoModels },
    );
    await flush();

    let rows = qa(container, '[data-testid="auto-model-row"]');
    expect(rows.length, 'Auto candidate rows must render').toBe(2);
    expect(rows.map((r) => r.getAttribute('data-model'))).toEqual([
      'gemini-3.8-flash',
      'deepseek-v4-flash',
    ]);
    expect(rows[0].getAttribute('draggable')).toBe('true');

    dragOnto(rows[1], rows[0]);
    await flush();

    rows = qa(container, '[data-testid="auto-model-row"]');
    expect(rows.map((r) => r.getAttribute('data-model'))).toEqual([
      'deepseek-v4-flash',
      'gemini-3.8-flash',
    ]);
    // Reordering alone must not persist anything.
    expect(onUpdateAutoModels).not.toHaveBeenCalled();

    click(qb(container, 'auto-model-save'), 'auto-model-save');
    await flush();
    expect(onUpdateAutoModels).toHaveBeenCalledTimes(1);
    expect(onUpdateAutoModels.mock.calls[0][0]).toEqual([
      'deepseek-v4-flash',
      'gemini-3.8-flash',
    ]);
    app.unmount();
  });

  it('keeps keyboard/button reorder as the accessible alternative to dragging', async () => {
    const { app, container } = mountSection({
      adminWriteEnabled: true,
      autoModels: AUTO_MODELS,
      allModels: ALL_MODELS,
    });
    await flush();

    let rows = qa(container, '[data-testid="auto-model-row"]');
    expect(rows.length, 'Auto candidate rows must render').toBe(2);
    // First row cannot move up; second row can move up.
    expect(need(qb(rows[0], 'auto-model-up'), 'auto-model-up on the first row').disabled).toBe(true);
    expect(need(qb(rows[1], 'auto-model-up'), 'auto-model-up on the last row').disabled).toBe(false);
    expect(need(qb(rows[0], 'auto-model-down'), 'auto-model-down on the first row').disabled).toBe(false);
    expect(need(qb(rows[1], 'auto-model-down'), 'auto-model-down on the last row').disabled).toBe(true);

    click(qb(rows[1], 'auto-model-up'), 'auto-model-up on the last row');
    await flush();
    rows = qa(container, '[data-testid="auto-model-row"]');
    expect(rows.map((r) => r.getAttribute('data-model'))).toEqual([
      'deepseek-v4-flash',
      'gemini-3.8-flash',
    ]);
    app.unmount();
  });

  it('freezes every write affordance (drag, add, save, remove, move) when admin writes are closed', async () => {
    const onUpdateAutoModels = vi.fn();
    const { app, container } = mountSection(
      { adminWriteEnabled: false, autoModels: AUTO_MODELS, allModels: ALL_MODELS },
      { onUpdateAutoModels },
    );
    await flush();

    const rows = qa(container, '[data-testid="auto-model-row"]');
    expect(rows.length, 'Auto candidate rows must render').toBe(2);
    for (const row of rows) {
      expect(row.getAttribute('draggable')).toBe('false');
      expect(row.getAttribute('data-draggable-disabled')).toBe('true');
    }
    expect(need(qb(container, 'auto-model-save'), 'auto-model-save').disabled).toBe(true);
    for (const id of ['auto-model-up', 'auto-model-down', 'auto-model-remove']) {
      for (const el of qa(container, `[data-testid="${id}"]`)) {
        expect((el as HTMLButtonElement).disabled).toBe(true);
      }
    }
    // The picker trigger is not rendered at all on a read-only console.
    expect(q(container, 'auto-model-add-open')).toBeNull();

    // And a synthetic drag cannot reorder anything.
    dragOnto(rows[1], rows[0]);
    await flush();
    expect(
      qa(container, '[data-testid="auto-model-row"]').map((r) => r.getAttribute('data-model')),
    ).toEqual(['gemini-3.8-flash', 'deepseek-v4-flash']);
    expect(onUpdateAutoModels).not.toHaveBeenCalled();
    app.unmount();
  });

  it('keeps the empty state and the remove affordance intact', async () => {
    const onUpdateAutoModels = vi.fn();
    const { app, container } = mountSection(
      { adminWriteEnabled: true, autoModels: [], allModels: ALL_MODELS },
      { onUpdateAutoModels },
    );
    await flush();

    expect(text(container)).toContain('暂无自定义配置');
    expect(qa(container, '[data-testid="auto-model-row"]')).toHaveLength(0);

    // Remove works on a populated list.
    app.unmount();
    const second = mountSection(
      { adminWriteEnabled: true, autoModels: AUTO_MODELS, allModels: ALL_MODELS },
      { onUpdateAutoModels },
    );
    await flush();
    click(qb(second.container, 'auto-model-remove-1'), 'auto-model-remove-1');
    await flush();
    expect(
      qa(second.container, '[data-testid="auto-model-row"]').map((r) => r.getAttribute('data-model')),
    ).toEqual(['gemini-3.8-flash']);
    expect(onUpdateAutoModels).not.toHaveBeenCalled();
    second.app.unmount();
  });

  it('ignores a dragstart that the browser starts outside a row', async () => {
    const { app, container } = mountSection({
      adminWriteEnabled: true,
      autoModels: AUTO_MODELS,
      allModels: ALL_MODELS,
    });
    await flush();

    dispatchDrag(container, 'dragstart');
    dispatchDrag(need(qa(container, '[data-testid="auto-model-row"]')[1], 'second Auto row'), 'drop');
    await flush();

    expect(
      qa(container, '[data-testid="auto-model-row"]').map((r) => r.getAttribute('data-model')),
    ).toEqual(['gemini-3.8-flash', 'deepseek-v4-flash']);
    app.unmount();
  });
});