// @vitest-environment happy-dom
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
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
/**
 * 模型更新表单的红相契约（对应线上 503 根因链第 4 环）。
 *
 * `ModelSubSection.handleSubmit()` 写的是 `await emit('update', ...)`，但 Vue 的
 * `emit` 返回 `void` —— 父层 async 处理函数的 Promise 永远不会被 await。
 * 真实调用方 `GovernanceView.editModel` 是 async 函数，因此拒绝（后端 412/403/
 * 网络失败）只会变成一个**无人 await 的 rejected Promise**：
 *   - `save()` 的 try/catch 捕获不到任何错误；
 *   - 第 575 行 `cancelForm()` 无条件执行 → 表单静默关闭；
 *   - 用户误以为保存成功（"我明明选了 responses 却没生效"）。
 *
 * 本组用例锁定"父层 update 处理函数的失败必须可被观察"这一契约：
 *   - B1  异步拒绝：表单不得自动收起 + 必须显示错误文案 + 不得吞异常逃逸
 *   - B1b 同步抛错：同上（当前 dev 构建下 Vue 会重抛，故当前为绿，属回归锁定）
 *   - B2  成功路径：表单正常收起且无错误横幅
 *   - B3  协议字段原样透传（选中透传值，取消选择透传空串）
 */
describe('ModelSubSection model update form (awaited emit contract)', () => {
  const EDITED_MODEL = 'gpt-4o';
  /** UiCollapsible 的 leave 过渡时长（`duration-200`）+ 余量。 */
  const LEAVE_TRANSITION_SETTLE_MS = 600;
  const POLL_INTERVAL_MS = 25;

  /** 收集逃逸出组件的 unhandledRejection；修复后相关条目必须为空（禁止吞异常放行）。 */
  let escapedRejections: unknown[] = [];
  const captureRejection = (reason: unknown) => {
    escapedRejections.push(reason);
  };

  beforeEach(() => {
    // 本组用例可独立运行（`-t` 过滤时外层 beforeEach 不执行），故自带 mock 兜底。
    upstreamImpl = async () => ({ provider: 'openai', source: 'upstream', models: [] });
    proxyStatusImpl = async () => ({
      available: true,
      proxy_url: 'http://127.0.0.1:8899',
      proxy_type: 'pproxy',
      description: '本地代理',
      hint: '',
    });
    escapedRejections = [];
    process.on('unhandledRejection', captureRejection);
  });

  afterEach(() => {
    process.off('unhandledRejection', captureRejection);
  });

  /** 排空微任务 + Vue 渲染队列（纯微任务，确定性，无 sleep 盲猜）。 */
  async function flush(rounds = 6) {
    for (let i = 0; i < rounds; i += 1) {
      await Promise.resolve();
      await nextTick();
    }
  }

  /** 让 Node 派发 unhandledRejection 诊断事件（单个事件循环轮次）。 */
  function nextEventLoopTurn(): Promise<void> {
    return new Promise((resolve) => {
      setImmediate(resolve);
    });
  }

  function mountEditForm(props: Record<string, unknown>, models: ModelView[] = mockModels) {
    const container = document.createElement('div');
    document.body.appendChild(container);
    const app = createApp(ModelSubSection, {
      providerName: 'openai',
      models,
      adminWriteEnabled: true,
      defaultExpanded: true,
      ...props,
    });
    app.mount(container);
    return { container, app };
  }

  /** 定位内联编辑面板容器（形如「编辑模型: gpt-4o」标题所在的区块）。 */
  function editPanel(container: HTMLElement): HTMLElement {
    const header = Array.from(container.querySelectorAll('span')).find((el) =>
      (el.textContent || '').trim().startsWith('编辑模型:'),
    );
    expect(header, '编辑面板标题必须存在').toBeTruthy();
    const panel = (header as HTMLElement).closest('div.space-y-4');
    expect(panel, '编辑面板容器必须存在').toBeTruthy();
    return panel as HTMLElement;
  }

  function inEdit<T extends Element>(container: HTMLElement, selector: string): T {
    const el = editPanel(container).querySelector(selector);
    expect(el, `编辑面板必须包含 ${selector}`).toBeTruthy();
    return el as unknown as T;
  }

  /** v-show 以 inline `display:none` 折叠，逐级向上判定可见性。 */
  function isShown(el: Element | null): boolean {
    if (!el) return false;
    let cur: Element | null = el;
    while (cur) {
      const style = (cur as HTMLElement).style;
      if (style && style.display === 'none') return false;
      cur = cur.parentElement;
    }
    return true;
  }

  /** 模型编辑表单是否仍处于展开（未收起）状态。 */
  function editFormOpen(container: HTMLElement): boolean {
    return isShown(inEdit(container, '[data-testid="update-model-btn"]'));
  }

  function editPanelText(container: HTMLElement): string {
    return editPanel(container).textContent || '';
  }

  /** 展开内联编辑表单，并断言它确实处于打开状态。 */
  async function openEditForm(container: HTMLElement) {
    const btn = container.querySelector('[data-testid="edit-model-btn"]');
    expect(btn, '模型行必须有编辑按钮').toBeTruthy();
    (btn as HTMLButtonElement).click();
    await flush();
    expect(editFormOpen(container), '点击编辑后表单必须展开').toBe(true);
  }

  /** 点击「更新」并等待提交链路与过渡动画全部落地。 */
  async function submitEditForm(container: HTMLElement, waitForCollapse: boolean) {
    inEdit<HTMLButtonElement>(container, '[data-testid="update-model-btn"]').click();
    await flush();
    await nextEventLoopTurn();
    if (waitForCollapse) {
      await vi.waitFor(
        () => {
          expect(editFormOpen(container), 'update 成功后编辑表单应收起').toBe(false);
        },
        { timeout: 2000, interval: POLL_INTERVAL_MS },
      );
    }
  }

  /**
   * 在 leave 过渡窗口内持续轮询：表单必须**始终**保持展开。
   * 只断言"某一瞬间还没收起"会被过渡动画蒙混过关，因此按 baseline+margin 轮询。
   */
  async function expectFormStaysOpen(container: HTMLElement) {
    const deadline = Date.now() + LEAVE_TRANSITION_SETTLE_MS;
    do {
      expect(
        editFormOpen(container),
        'update 失败后编辑表单在过渡窗口内被收起了 —— 用户会误以为保存成功',
      ).toBe(true);
      await new Promise((resolve) => setTimeout(resolve, POLL_INTERVAL_MS));
    } while (Date.now() < deadline);
  }

  /** 仅挑出本次 update 相关的逃逸拒绝，屏蔽无关噪声。 */
  function escapedFor(fragment: string): unknown[] {
    return escapedRejections.filter((e) =>
      String((e as Error)?.message || e).includes(fragment),
    );
  }

  it('B1: a rejected update handler keeps the form open and surfaces the error', async () => {
    // Arrange
    const received: Array<{ name: string; payload: any }> = [];
    const { container, app } = mountEditForm({
      onUpdate: async (name: string, payload: any) => {
        received.push({ name, payload });
        // 模拟后端 412（配置版本冲突）这类真实失败。
        throw new Error('412 Precondition Failed: config version conflict');
      },
    });
    await flush();
    await openEditForm(container);
    escapedRejections = [];

    // Act
    inEdit<HTMLButtonElement>(container, '[data-testid="update-model-btn"]').click();
    await flush();
    await nextEventLoopTurn();

    // Assert — 父层处理函数确实被调用过（用例不是恒真空转）
    expect(received).toHaveLength(1);
    expect(received[0].name).toBe(EDITED_MODEL);

    // Assert — 失败必须可观察：错误文案出现在编辑表单内
    await vi.waitFor(
      () => {
        expect(
          editPanelText(container),
          'update 失败时必须在编辑表单内显示错误文案（当前实现静默关闭表单，用户误以为保存成功）',
        ).toContain('412 Precondition Failed');
      },
      { timeout: 2000, interval: POLL_INTERVAL_MS },
    );

    // Assert — 失败时表单不得自动收起（覆盖整个 leave 过渡窗口）
    await expectFormStaysOpen(container);

    // Assert — 失败不得以 unhandledRejection 形式逃逸出组件（禁止吞异常放行）
    expect(
      escapedFor('412'),
      'update 的失败必须在组件内部被 await 并处理，不得逃逸成 unhandledRejection',
    ).toEqual([]);

    app.unmount();
    document.body.removeChild(container);
  });

  it('B1b: a synchronously throwing update handler keeps the form open and surfaces the error', async () => {
    // Arrange
    const received: Array<{ name: string; payload: any }> = [];
    const { container, app } = mountEditForm({
      onUpdate: ((name: string, payload: any) => {
        received.push({ name, payload });
        // 同步抛错（部分调用方未走 async 时会发生）。
        throw new Error('403 Forbidden: admin write disabled');
      }) as any,
    });
    await flush();
    await openEditForm(container);
    escapedRejections = [];

    // Act
    inEdit<HTMLButtonElement>(container, '[data-testid="update-model-btn"]').click();
    await flush();
    await nextEventLoopTurn();

    // Assert
    expect(received).toHaveLength(1);
    expect(received[0].name).toBe(EDITED_MODEL);

    await vi.waitFor(
      () => {
        expect(editPanelText(container), '同步抛错时必须在编辑表单内显示错误文案').toContain(
          '403 Forbidden',
        );
      },
      { timeout: 2000, interval: POLL_INTERVAL_MS },
    );

    await expectFormStaysOpen(container);

    expect(
      escapedFor('403'),
      '同步抛错不得逃逸成 unhandledRejection',
    ).toEqual([]);

    app.unmount();
    document.body.removeChild(container);
  });

  it('B2: a successful update collapses the form and shows no error banner', async () => {
    // Arrange
    const received: Array<{ name: string; payload: any }> = [];
    const { container, app } = mountEditForm({
      onUpdate: async (name: string, payload: any) => {
        received.push({ name, payload });
      },
    });
    await flush();
    await openEditForm(container);
    escapedRejections = [];

    // Act（内部以条件轮询等待表单收起）
    await submitEditForm(container, true);

    // Assert
    expect(received).toHaveLength(1);
    expect(received[0].name).toBe(EDITED_MODEL);
    expect(editFormOpen(container), 'update 成功后编辑表单必须收起').toBe(false);
    expect(editPanelText(container), '成功路径不得显示错误文案').not.toContain('Failed');
    expect(editPanelText(container), '成功路径不得显示错误文案').not.toContain('Forbidden');
    expect(escapedFor(EDITED_MODEL), '成功路径不得产生 unhandledRejection').toEqual([]);

    app.unmount();
    document.body.removeChild(container);
  });

  it('B3: passes the selected protocol through verbatim', async () => {
    // Arrange: 现有模型已声明 protocol='chat'，高级面板因此默认展开。
    const protoModels: ModelView[] = [{ ...mockModels[0], protocol: 'chat' }];
    const received: Array<{ name: string; payload: any }> = [];
    const { container, app } = mountEditForm(
      {
        onUpdate: async (name: string, payload: any) => {
          received.push({ name, payload });
        },
      },
      protoModels,
    );
    await flush();
    await openEditForm(container);

    // Assert — 回显的是已声明的 chat
    expect(
      inEdit(container, '[data-testid="model-proto-chat"]').className,
      '打开编辑表单时应回显模型已声明的 protocol=chat',
    ).toContain('bg-slate-900');
    expect(
      inEdit(container, '[data-testid="model-proto-responses"]').className,
      'responses 初始不应处于选中态',
    ).not.toContain('bg-slate-900');

    // Act — 切到 responses 并提交
    inEdit<HTMLButtonElement>(container, '[data-testid="model-proto-responses"]').click();
    await flush();
    expect(
      inEdit(container, '[data-testid="model-proto-responses"]').className,
      '点击后 responses 应变为选中态',
    ).toContain('bg-slate-900');
    expect(
      inEdit(container, '[data-testid="model-proto-chat"]').className,
      '点击后 chat 应取消选中态（协议为单选语义）',
    ).not.toContain('bg-slate-900');

    await submitEditForm(container, true);

    // Assert — 载荷中原样透传 'responses'
    expect(received).toHaveLength(1);
    expect(received[0].name).toBe(EDITED_MODEL);
    expect(received[0].payload.protocol).toBe('responses');

    app.unmount();
    document.body.removeChild(container);
  });

  it('B3b: passes an empty string when the protocol selection is cleared', async () => {
    // Arrange: 模型未声明 protocol -> 打开编辑表单时无协议处于选中态。
    const received: Array<{ name: string; payload: any }> = [];
    const { container, app } = mountEditForm({
      onUpdate: async (name: string, payload: any) => {
        received.push({ name, payload });
      },
    });
    await flush();
    await openEditForm(container);

    for (const opt of ['chat', 'messages', 'responses'] as const) {
      expect(
        inEdit(container, `[data-testid="model-proto-${opt}"]`).className,
        `未声明 protocol 时 ${opt} 不应处于选中态`,
      ).not.toContain('bg-slate-900');
    }

    // Act + Assert — 选中再取消，载荷必须回到空串（= 继承服务商默认协议）
    inEdit<HTMLButtonElement>(container, '[data-testid="model-proto-responses"]').click();
    await flush();
    expect(
      inEdit(container, '[data-testid="model-proto-responses"]').className,
      '点击后 responses 应变为选中态',
    ).toContain('bg-slate-900');
    inEdit<HTMLButtonElement>(container, '[data-testid="model-proto-responses"]').click();
    await flush();
    expect(
      inEdit(container, '[data-testid="model-proto-responses"]').className,
      '再次点击同一协议应取消选择',
    ).not.toContain('bg-slate-900');

    await submitEditForm(container, true);

    expect(received).toHaveLength(1);
    expect(received[0].name).toBe(EDITED_MODEL);
    expect(
      received[0].payload.protocol,
      '取消选择必须发送空字符串以表示「继承服务商默认协议」，不得发送 undefined/残留值',
    ).toBe('');

    app.unmount();
    document.body.removeChild(container);
  });
});