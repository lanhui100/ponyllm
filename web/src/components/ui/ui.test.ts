// @vitest-environment happy-dom
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { createApp, nextTick, h } from 'vue';
import Icons from './Icons.vue';
import UiTooltip from './UiTooltip.vue';
import UiButton from './UiButton.vue';
import UiBadge from './UiBadge.vue';
import UiCollapsible from './UiCollapsible.vue';

describe('UI Primitives & Modern Design System', () => {
  let container: HTMLDivElement;

  beforeEach(() => {
    container = document.createElement('div');
    document.body.appendChild(container);
  });

  afterEach(() => {
    document.body.removeChild(container);
  });

  it('renders SVG icon correctly', async () => {
    const app = createApp({
      render: () => h(Icons, { name: 'zap', size: 18 }),
    });
    app.mount(container);
    await nextTick();

    const svg = container.querySelector('svg');
    expect(svg).not.toBeNull();
    expect(svg?.getAttribute('width')).toBe('18px');
    app.unmount();
  });

  it('renders button with variants and handles click', async () => {
    let clicked = false;
    const app = createApp({
      render: () =>
        h(
          UiButton,
          {
            variant: 'ghost',
            size: 'icon',
            onClick: () => {
              clicked = true;
            },
          },
          () => 'Click me'
        ),
    });
    app.mount(container);
    await nextTick();

    const btn = container.querySelector('button');
    expect(btn?.className).toContain('bg-transparent');
    btn?.click();
    expect(clicked).toBe(true);
    app.unmount();
  });

  it('renders badge with soft tint variants', async () => {
    const app = createApp({
      render: () => h(UiBadge, { variant: 'success' }, () => 'Active'),
    });
    app.mount(container);
    await nextTick();

    const badge = container.querySelector('span');
    expect(badge?.className).toContain('bg-emerald-600');
    expect(badge?.className).toContain('text-white');
    expect(badge?.textContent).toBe('Active');
    app.unmount();
  });

  it('renders tooltip on hover', async () => {
    const app = createApp({
      render: () =>
        h(
          UiTooltip,
          { content: '测速探针说明' },
          () => h('button', { id: 'target' }, 'Hover target')
        ),
    });
    app.mount(container);
    await nextTick();

    const target = container.querySelector('#target');
    expect(target).not.toBeNull();

    // Trigger mouseenter on parent container
    const wrapper = container.firstElementChild as HTMLElement;
    wrapper.dispatchEvent(new MouseEvent('mouseenter'));

    // Wait for tooltip delay (120ms)
    await new Promise((r) => setTimeout(r, 150));
    await nextTick();

    const tip = document.body.querySelector('[data-testid="ui-tooltip"]') || container.querySelector('[data-testid="ui-tooltip"]');
    expect(tip).not.toBeNull();
    expect(tip?.textContent?.trim()).toBe('测速探针说明');
    app.unmount();
  });

  it('renders collapsible container according to open prop', async () => {
    const app = createApp({
      render: () =>
        h(
          UiCollapsible,
          { open: true },
          () => h('div', { id: 'inner' }, 'Inner Content')
        ),
    });
    app.mount(container);
    await nextTick();

    const inner = container.querySelector('#inner');
    expect(inner).not.toBeNull();
    expect(inner?.textContent).toBe('Inner Content');
    app.unmount();
  });
});
