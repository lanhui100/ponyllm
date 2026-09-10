// @vitest-environment happy-dom
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { createApp, nextTick, h } from 'vue';
import Icons from './Icons.vue';
import UiTooltip from './UiTooltip.vue';
import UiButton from './UiButton.vue';
import UiBadge from './UiBadge.vue';
import UiCollapsible from './UiCollapsible.vue';
import UiToast from './UiToast.vue';
import { toast, useToast } from '../../composables/useToast';

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

  it('renders center glassmorphism toast with semantic colors and icons', async () => {
    const app = createApp({
      render: () => h(UiToast),
    });
    app.mount(container);
    await nextTick();

    toast.success('配置同步成功');
    await nextTick();

    const toastEl = container.querySelector('[data-testid="ui-toast"]');
    expect(toastEl).not.toBeNull();
    // 居中定位类
    expect(toastEl?.className).toContain('top-1/2');
    expect(toastEl?.className).toContain('left-1/2');
    expect(toastEl?.className).toContain('-translate-x-1/2');
    expect(toastEl?.className).toContain('-translate-y-1/2');

    // 文本与图标
    const msgEl = container.querySelector('[data-testid="toast-message"]');
    expect(msgEl?.textContent?.trim()).toBe('配置同步成功');
    const iconWrap = container.querySelector('[data-testid="toast-icon-wrap"]');
    expect(iconWrap?.className).toContain('text-emerald-700');

    // 手动关闭
    const closeBtn = container.querySelector('[data-testid="toast-close-btn"]') as HTMLButtonElement;
    expect(closeBtn).not.toBeNull();
    closeBtn.click();
    await nextTick();

    const { currentToast } = useToast();
    expect(currentToast.value).toBeNull();
    app.unmount();
  });

  it('handles confirm dialog mode with secondary confirmation promise resolution', async () => {
    const app = createApp({
      render: () => h(UiToast),
    });
    app.mount(container);
    await nextTick();

    let confirmResult: boolean | null = null;
    const confirmPromise = toast.confirm({
      title: '确认删除密钥',
      message: '确定要删除该密钥吗？',
      confirmText: '彻底删除',
      variant: 'destructive',
    }).then((res) => {
      confirmResult = res;
      return res;
    });

    await nextTick();

    // 遮罩层出现
    const overlay = container.querySelector('[data-testid="toast-overlay"]');
    expect(overlay).not.toBeNull();

    // 检查按钮与文案
    const titleEl = container.querySelector('[data-testid="toast-title"]');
    expect(titleEl?.textContent).toContain('确认删除密钥');
    const okBtn = container.querySelector('[data-testid="toast-ok-btn"]') as HTMLButtonElement;
    expect(okBtn).not.toBeNull();
    expect(okBtn.textContent).toContain('彻底删除');

    // 点击确定
    okBtn.click();
    await confirmPromise;
    expect(confirmResult).toBe(true);

    await nextTick();
    const { currentToast } = useToast();
    expect(currentToast.value).toBeNull();
    app.unmount();
  });
});
