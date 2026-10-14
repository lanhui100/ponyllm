// @vitest-environment happy-dom
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { createApp, nextTick } from 'vue';
import { createPinia, setActivePinia } from 'pinia';
import TokensView from './TokensView.vue';
import * as userApi from '../lib/userApi';

vi.mock('../components/NavBar.vue', () => ({
  default: {
    name: 'NavBar',
    template: '<nav data-testid="mock-navbar"></nav>',
  },
}));

describe('TokensView.vue', () => {
  let pinia: ReturnType<typeof createPinia>;
  let container: HTMLDivElement;
  let app: ReturnType<typeof createApp>;
  /** Vue 组件内未捕获错误（"不得抛未捕获异常"的机器凭证）。 */
  let vueErrors: unknown[];

  const dummyTokens: userApi.UserTokenMeta[] = [
    {
      key_id: 'tk-1',
      name: 'dev-token',
      enabled: true,
      model_limits: ['gpt-4o', 'claude-3-5-sonnet'],
      quota: 50000,
      expires_at: null,
      used_tokens: 1200,
    },
    {
      key_id: 'tk-2',
      name: 'ci-token',
      enabled: false,
      model_limits: null,
      quota: null,
      expires_at: null,
      used_tokens: 300,
    },
  ];

  // wave-6：模型多选下拉的数据源（display_name 不含 a/b/c，保证按 id 定位选项无歧义）
  const dummyModels: userApi.UserModelOption[] = [
    { id: 'a', provider: 'ponyllm', display_name: 'One' },
    { id: 'b', provider: 'openai', display_name: 'Two' },
    { id: 'c', provider: 'anthropic', display_name: 'Three' },
  ];

  beforeEach(() => {
    vi.clearAllMocks();
    pinia = createPinia();
    setActivePinia(pinia);
    container = document.createElement('div');
    document.body.appendChild(container);
    vueErrors = [];

    vi.spyOn(userApi, 'listMyTokens').mockResolvedValue([...dummyTokens]);
    vi.spyOn(userApi, 'listAvailableModels').mockResolvedValue([...dummyModels]);
    vi.spyOn(userApi, 'createMyToken').mockResolvedValue({
      key_id: 'tk-new',
      api_key: 'sk-pony-new-created-secret-1234',
      name: 'new-token',
      model_limits: ['gpt-4o'],
      quota: 100000,
      expires_at: null,
      enabled: true,
      used_tokens: 0,
    });
    vi.spyOn(userApi, 'rotateMyToken').mockResolvedValue({
      api_key: 'sk-pony-rotated-secret-5678',
    });
    vi.spyOn(userApi, 'updateMyToken').mockResolvedValue(dummyTokens[0]);
    vi.spyOn(userApi, 'deleteMyToken').mockResolvedValue(undefined);

    Object.defineProperty(navigator, 'clipboard', {
      value: {
        writeText: vi.fn().mockResolvedValue(undefined),
      },
      writable: true,
      configurable: true,
    });
  });

  afterEach(() => {
    if (app) {
      app.unmount();
    }
    if (container && container.parentNode) {
      document.body.removeChild(container);
    }
  });

  async function mountView() {
    app = createApp(TokensView);
    app.config.errorHandler = (err) => {
      vueErrors.push(err);
    };
    app.use(pinia);
    app.mount(container);
    // 等待 onMounted 加载完成
    await nextTick();
    await nextTick();
    await new Promise((resolve) => setTimeout(resolve, 50));
  }

  function openCreateModal(): void {
    const openBtn = container.querySelector(
      '[data-testid="open-create-token-btn"]',
    ) as HTMLButtonElement;
    openBtn.click();
  }

  // --- wave-6「允许模型」多选下拉 helper（契约 §2.3 冻结 testid） ---
  function modelTrigger(): HTMLButtonElement | null {
    return container.querySelector(
      '[data-testid="token-models-select"]',
    ) as HTMLButtonElement | null;
  }

  function modelOptions(): HTMLElement[] {
    return Array.from(
      container.querySelectorAll('[data-testid="token-models-option"]'),
    ) as HTMLElement[];
  }

  function openModelPicker(): void {
    const trigger = modelTrigger();
    expect(
      trigger,
      '契约 §2.3：必须存在 data-testid="token-models-select" 触发器（红相：仍是自由文本输入 → FAIL）',
    ).not.toBeNull();
    trigger!.click();
  }

  function selectModel(id: string): void {
    if (modelOptions().length === 0) {
      openModelPicker();
    }
    const options = modelOptions();
    const opt =
      options.find((el) => (el.textContent ?? '').trim() === id) ??
      options.find((el) => (el.textContent ?? '').includes(id));
    expect(
      opt,
      `契约 §2.3：展开后必须存在 data-testid="token-models-option" 且含模型 ${id}；` +
        `当前选项文本 = ${JSON.stringify(options.map((o) => o.textContent))}`,
    ).toBeDefined();
    opt!.click();
  }

  function modelChips(): HTMLElement[] {
    return Array.from(
      container.querySelectorAll('[data-testid="token-models-chip"]'),
    ) as HTMLElement[];
  }

  function removeFirstChip(): void {
    const chips = modelChips();
    expect(chips.length, '取消选中前必须存在 data-testid="token-models-chip"').toBeGreaterThan(0);
    const removeBtn = chips[0].querySelector('button') as HTMLButtonElement | null;
    (removeBtn ?? chips[0]).click();
  }

  it('renders token list, does NOT render embedded create form by default', async () => {
    await mountView();

    expect(container.textContent).toContain('我的 Token');
    expect(container.textContent).toContain('Token 列表');
    expect(container.textContent).toContain('dev-token');
    expect(container.textContent).toContain('ci-token');

    // 页面默认不展示新增编辑弹窗
    expect(container.querySelector('[data-testid="create-token-modal"]')).toBeNull();

    // 列表下方存在“新增 Token”按钮
    const openBtn = container.querySelector('[data-testid="open-create-token-btn"]') as HTMLButtonElement | null;
    expect(openBtn).not.toBeNull();
    expect(openBtn?.textContent).toContain('新增 Token');

    // 检查不再包含停用/启用状态徽标与切换按钮
    expect(container.textContent).not.toContain('停用');
    expect(container.textContent).not.toContain('启用');

    // 旋转已改为轮换
    const rotateBtns = container.querySelectorAll('[data-testid="rotate-token-btn"]');
    expect(rotateBtns.length).toBe(2);
    expect(rotateBtns[0].textContent).toContain('轮换');
    expect(container.textContent).not.toContain('旋转');
  });

  it('opens create modal on click, closes on cancel', async () => {
    await mountView();

    const openBtn = container.querySelector('[data-testid="open-create-token-btn"]') as HTMLButtonElement;
    openBtn.click();
    await nextTick();

    expect(container.querySelector('[data-testid="create-token-modal"]')).not.toBeNull();
    expect(container.querySelector('[data-testid="token-name-input"]')).not.toBeNull();

    // 点击取消
    const buttons = Array.from(container.querySelectorAll('button'));
    const cancelBtn = buttons.find((b) => b.textContent?.includes('取消'));
    expect(cancelBtn).toBeDefined();
    cancelBtn!.click();
    await nextTick();

    expect(container.querySelector('[data-testid="create-token-modal"]')).toBeNull();
  });

  // -------------------------------------------------------------------------
  // wave-6 新增：允许模型多选下拉契约（T-F5 / T-F6）
  // -------------------------------------------------------------------------
  it('renders model multi-select with frozen testids, placeholder and chips (wave-6)', async () => {
    await mountView();
    openCreateModal();
    await nextTick();

    // T-F5：触发器存在 + 零选中时占位文案逐字为「留空为全部模型」
    const trigger = modelTrigger();
    expect(trigger).not.toBeNull();
    expect(trigger!.textContent).toContain('留空为全部模型');
    // 旧自由文本输入必须已被替换
    expect(container.querySelector('[data-testid="token-models-input"]')).toBeNull();

    // 展开后：搜索框 + 选项
    openModelPicker();
    await nextTick();
    expect(
      container.querySelector('[data-testid="token-models-search"]'),
      '契约 §2.3：展开后必须有 data-testid="token-models-search" 搜索框',
    ).not.toBeNull();
    expect(modelOptions().length).toBe(dummyModels.length);

    // 选中一项 → chip 出现
    selectModel('a');
    await nextTick();
    expect(modelChips().length).toBe(1);

    // 搜索过滤：输入 'b' 后仅剩 id 含 'b' 的选项
    const search = container.querySelector(
      '[data-testid="token-models-search"]',
    ) as HTMLInputElement;
    search.value = 'b';
    search.dispatchEvent(new Event('input'));
    await nextTick();
    expect(modelOptions().length).toBe(1);
    expect(modelOptions()[0].textContent).toContain('b');

    expect(vueErrors).toEqual([]);
  });

  it('saves new token: closes create modal, displays token result modal with copy button', async () => {
    await mountView();

    // 打开编辑弹窗
    const openBtn = container.querySelector('[data-testid="open-create-token-btn"]') as HTMLButtonElement;
    openBtn.click();
    await nextTick();

    // 填写表单
    const nameInput = container.querySelector('[data-testid="token-name-input"]') as HTMLInputElement;
    const quotaInput = container.querySelector('[data-testid="token-quota-input"]') as HTMLInputElement;

    nameInput.value = 'test-agent-token';
    nameInput.dispatchEvent(new Event('input'));

    quotaInput.value = '100000';
    quotaInput.dispatchEvent(new Event('input'));

    // wave-6：多选下拉选两项（替换原自由文本 'gpt-4o'）
    selectModel('a');
    await nextTick();
    selectModel('b');
    await nextTick();

    // 保存
    const submitBtn = container.querySelector('[data-testid="submit-create-token-btn"]') as HTMLButtonElement;
    submitBtn.click();
    await nextTick();
    await new Promise((resolve) => setTimeout(resolve, 50));

    expect(userApi.createMyToken).toHaveBeenCalledWith({
      name: 'test-agent-token',
      quota: 100000,
      model_limits: ['a', 'b'],
    });

    // 编辑弹窗已关闭
    expect(container.querySelector('[data-testid="create-token-modal"]')).toBeNull();

    // 弹出精简后的结果展示弹窗
    const resultModal = container.querySelector('[data-testid="token-result-modal"]');
    expect(resultModal).not.toBeNull();
    expect(resultModal?.textContent).toContain('Token 创建成功');
    expect(resultModal?.textContent).toContain('明文仅展示一次');
    expect(resultModal?.textContent).not.toContain('安全提醒');

    // 检查明文输入框与复制按钮
    const input = container.querySelector('[data-testid="plaintext-token-input"]') as HTMLInputElement;
    expect(input).not.toBeNull();
    expect(input.value).toBe('sk-pony-new-created-secret-1234');

    const copyBtn = container.querySelector('[data-testid="copy-token-btn"]') as HTMLButtonElement;
    expect(copyBtn).not.toBeNull();
    expect(copyBtn.textContent).toContain('复制');

    // 点击复制
    copyBtn.click();
    await new Promise((resolve) => setTimeout(resolve, 50));
    await nextTick();
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith('sk-pony-new-created-secret-1234');
    expect(copyBtn.textContent).toContain('已复制');

    // 关闭展示弹窗
    const closeResultBtn = container.querySelector('[data-testid="close-result-modal-btn"]') as HTMLButtonElement;
    closeResultBtn.click();
    await nextTick();
    expect(container.querySelector('[data-testid="token-result-modal"]')).toBeNull();

    expect(vueErrors).toEqual([]);
  });

  it('submits model_limits: undefined when all selected models are deselected (wave-6 T-F6)', async () => {
    await mountView();
    openCreateModal();
    await nextTick();

    const nameInput = container.querySelector('[data-testid="token-name-input"]') as HTMLInputElement;
    nameInput.value = 'no-limit-token';
    nameInput.dispatchEvent(new Event('input'));

    // 先选中，再通过 chip 移除按钮全部取消
    selectModel('a');
    await nextTick();
    expect(modelChips().length).toBe(1);
    removeFirstChip();
    await nextTick();
    expect(modelChips().length).toBe(0);

    const submitBtn = container.querySelector('[data-testid="submit-create-token-btn"]') as HTMLButtonElement;
    submitBtn.click();
    await nextTick();
    await new Promise((resolve) => setTimeout(resolve, 50));

    expect(userApi.createMyToken).toHaveBeenCalledTimes(1);
    const arg = vi.mocked(userApi.createMyToken).mock.calls[0][0];
    expect(
      arg.model_limits,
      '契约 §2.4：零选中必须提交 undefined（严禁空数组 [] —— Some([]) 语义为全部拒绝）',
    ).toBeUndefined();
    expect(
      JSON.stringify(arg),
      '契约 §2.4：零选中时 model_limits 键不得出现于 JSON 请求体',
    ).not.toContain('model_limits');
    expect(vueErrors).toEqual([]);
  });

  it('model list load failure does not block token creation (wave-6 T-F7 优雅降级)', async () => {
    // Arrange：模型列表加载失败
    vi.mocked(userApi.listAvailableModels).mockRejectedValue(new Error('models unavailable'));

    await mountView();
    openCreateModal();
    await nextTick();
    await new Promise((resolve) => setTimeout(resolve, 50));

    // 触发器仍在（下拉字段存在，失败不应把字段整块删掉）
    expect(modelTrigger()).not.toBeNull();

    // 名称可填、提交按钮未被禁用
    const nameInput = container.querySelector('[data-testid="token-name-input"]') as HTMLInputElement;
    nameInput.value = 'degraded-token';
    nameInput.dispatchEvent(new Event('input'));
    await nextTick();

    const submitBtn = container.querySelector('[data-testid="submit-create-token-btn"]') as HTMLButtonElement;
    expect(submitBtn.disabled, '契约 §2.5：模型列表失败不得禁用提交按钮').toBe(false);

    // Act：降级路径下未选择任何模型 → 照常提交
    submitBtn.click();
    await nextTick();
    await new Promise((resolve) => setTimeout(resolve, 50));

    // Assert
    expect(userApi.createMyToken).toHaveBeenCalledTimes(1);
    const arg = vi.mocked(userApi.createMyToken).mock.calls[0][0];
    expect(arg.name).toBe('degraded-token');
    expect(arg.model_limits).toBeUndefined();
    expect(container.querySelector('[data-testid="create-token-modal"]')).toBeNull();
    expect(container.querySelector('[data-testid="token-result-modal"]')).not.toBeNull();
    expect(vueErrors, `降级路径不得抛未捕获异常：${JSON.stringify(vueErrors)}`).toEqual([]);
  });

  it('rotates token: triggers rotate API and displays result modal with new token and copy button', async () => {
    await mountView();

    const rotateBtn = container.querySelectorAll('[data-testid="rotate-token-btn"]')[0] as HTMLButtonElement;
    rotateBtn.click();
    await nextTick();
    await new Promise((resolve) => setTimeout(resolve, 50));

    expect(userApi.rotateMyToken).toHaveBeenCalledWith('tk-1');

    // 弹出结果展示弹窗
    const resultModal = container.querySelector('[data-testid="token-result-modal"]');
    expect(resultModal).not.toBeNull();
    expect(resultModal?.textContent).toContain('Token 轮换成功');
    expect(resultModal?.textContent).not.toContain('安全提醒');
    expect(resultModal?.textContent).not.toContain('旧凭证已即刻作废失效');

    const input = container.querySelector('[data-testid="plaintext-token-input"]') as HTMLInputElement;
    expect(input.value).toBe('sk-pony-rotated-secret-5678');

    const copyBtn = container.querySelector('[data-testid="copy-token-btn"]') as HTMLButtonElement;
    copyBtn.click();
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith('sk-pony-rotated-secret-5678');
  });
});
