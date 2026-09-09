// @vitest-environment happy-dom
import { describe, it, expect } from 'vitest';
import { createApp, nextTick } from 'vue';
import UpstreamModelPicker from './UpstreamModelPicker.vue';

function mountPicker() {
  const container = document.createElement('div');
  document.body.appendChild(container);
  let confirmed: string[][] = [];
  const app = createApp(UpstreamModelPicker, {
    providerName: 'openai',
    models: [{ id: 'gpt-4o' }, { id: 'gpt-4o-mini' }, { id: 'o3' }],
    existingNames: ['gpt-4o'],
    submitting: false,
    progress: null,
    onConfirm: (ids: string[]) => confirmed.push(ids),
  });
  app.mount(container);
  return { container, app, confirmed };
}

describe('UpstreamModelPicker', () => {
  it('disables existing models, selects all selectable, confirms ids', async () => {
    const { container, app, confirmed } = mountPicker();
    await nextTick();

    // 已存在项禁用
    const existing = container.querySelector('[data-testid="picker-check-gpt-4o"]') as HTMLInputElement;
    expect(existing.disabled).toBe(true);

    // 全选仅选中未存在的两项
    (container.querySelector('[data-testid="picker-toggle-all"]') as HTMLButtonElement).click();
    await nextTick();
    (container.querySelector('[data-testid="picker-confirm-btn"]') as HTMLButtonElement).click();
    await nextTick();
    expect(confirmed).toEqual([['gpt-4o-mini', 'o3']]);

    app.unmount();
    document.body.removeChild(container);
  });

  it('filters by keyword', async () => {
    const { container, app } = mountPicker();
    await nextTick();

    const search = container.querySelector('[data-testid="picker-search-input"]') as HTMLInputElement;
    search.value = 'mini';
    search.dispatchEvent(new Event('input'));
    await nextTick();

    const list = container.querySelector('[data-testid="picker-model-list"]')!;
    expect(list.textContent).toContain('gpt-4o-mini');
    expect(list.textContent).not.toContain('o3');

    app.unmount();
    document.body.removeChild(container);
  });
});
