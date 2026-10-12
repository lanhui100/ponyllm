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

  beforeEach(() => {
    vi.clearAllMocks();
    pinia = createPinia();
    setActivePinia(pinia);
    container = document.createElement('div');
    document.body.appendChild(container);

    vi.spyOn(userApi, 'listMyTokens').mockResolvedValue([...dummyTokens]);
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
    app.use(pinia);
    app.mount(container);
    // 等待 onMounted 加载完成
    await nextTick();
    await nextTick();
    await new Promise((resolve) => setTimeout(resolve, 50));
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

  it('saves new token: closes create modal, displays token result modal with copy button', async () => {
    await mountView();

    // 打开编辑弹窗
    const openBtn = container.querySelector('[data-testid="open-create-token-btn"]') as HTMLButtonElement;
    openBtn.click();
    await nextTick();

    // 填写表单
    const nameInput = container.querySelector('[data-testid="token-name-input"]') as HTMLInputElement;
    const quotaInput = container.querySelector('[data-testid="token-quota-input"]') as HTMLInputElement;
    const modelsInput = container.querySelector('[data-testid="token-models-input"]') as HTMLInputElement;

    nameInput.value = 'test-agent-token';
    nameInput.dispatchEvent(new Event('input'));

    quotaInput.value = '100000';
    quotaInput.dispatchEvent(new Event('input'));

    modelsInput.value = 'gpt-4o';
    modelsInput.dispatchEvent(new Event('input'));

    await nextTick();

    // 保存
    const submitBtn = container.querySelector('[data-testid="submit-create-token-btn"]') as HTMLButtonElement;
    submitBtn.click();
    await nextTick();
    await new Promise((resolve) => setTimeout(resolve, 50));

    expect(userApi.createMyToken).toHaveBeenCalledWith({
      name: 'test-agent-token',
      quota: 100000,
      model_limits: ['gpt-4o'],
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
