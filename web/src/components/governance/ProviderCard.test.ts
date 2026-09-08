// @vitest-environment happy-dom
import { describe, it, expect } from 'vitest';
import { createApp, nextTick } from 'vue';
import ProviderCard from './ProviderCard.vue';
import type { ProviderView, ModelView, KeyView } from '../../types/admin';

describe('ProviderCard UI and Phase 2 Requirements', () => {
  const mockProvider: ProviderView = {
    name: 'test-provider',
    base_url: 'https://sensitive-tokens-url.ponyjob.top/v1?token=secret123',
    default_model: 'gpt-4o',
    strategy: 'round_robin',
    billing_mode: 'metered',
    input_price: 2.0,
    cached_price: 1.0,
    output_price: 5.0,
    models: 1,
    default_protocol: 'chat',
    chat_url: null,
    messages_url: null,
    responses_url: null,
  };

  const mockModels: ModelView[] = [
    {
      name: 'gpt-4o',
      tier: 'Smart',
      context_window: '128k',
      thinking_default: 'Off',
      thinking_max: 'High',
      provider: 'test-provider',
    },
  ];

  const mockKeys: KeyView[] = [
    {
      id: 'key-1',
      provider: 'test-provider',
      masked_key: 'sk-proj-****',
      priority: 1,
      weight: 10,
      state: 'active',
    },
  ];

  it('renders warm orange icon, strips plaintext sensitive URL, and removes billing params', async () => {
    const container = document.createElement('div');
    document.body.appendChild(container);

    const app = createApp(ProviderCard, {
      provider: mockProvider,
      models: mockModels,
      keys: mockKeys,
      adminWriteEnabled: true,
      keyTestResults: {},
      testingKeyIds: new Set<string>(),
      defaultExpanded: true,
    });
    app.mount(container);
    await nextTick();

    // 1. Warm orange icon
    const iconContainer = container.querySelector('.bg-orange-50');
    expect(iconContainer).not.toBeNull();
    expect(iconContainer?.className).toContain('text-orange-600');

    // 2. Sensitive base_url stripped from text under name
    expect(container.textContent).not.toContain('secret123');
    expect(container.textContent).not.toContain('https://sensitive-tokens-url.ponyjob.top/v1');

    // 3. Removed "计费单价与服务商高级参数"
    expect(container.textContent).not.toContain('计费单价与服务商高级参数');

    // 4. Protocols selector (non-dropdown pills)
    const protocolSection = container.querySelector('[data-testid="protocol-section"]');
    expect(protocolSection).not.toBeNull();
    const chatPill = container.querySelector('[data-testid="protocol-pill-chat"]');
    const messagesPill = container.querySelector('[data-testid="protocol-pill-messages"]');
    const responsesPill = container.querySelector('[data-testid="protocol-pill-responses"]');
    expect(chatPill).not.toBeNull();
    expect(messagesPill).not.toBeNull();
    expect(responsesPill).not.toBeNull();

    // 5. Renamed to "密钥" and "模型"
    expect(container.textContent).toContain('密钥 (1)');
    expect(container.textContent).toContain('模型 (1)');
    expect(container.textContent).not.toContain('密钥凭证');
    expect(container.textContent).not.toContain('挂载模型');

    app.unmount();
    document.body.removeChild(container);
  });

  it('handles protocol configuration, toggling pills, and emits update-provider with validation', async () => {
    const container = document.createElement('div');
    document.body.appendChild(container);

    let updatedPayload: any = null;
    const app = createApp(ProviderCard, {
      provider: mockProvider,
      models: mockModels,
      keys: mockKeys,
      adminWriteEnabled: true,
      keyTestResults: {},
      testingKeyIds: new Set<string>(),
      defaultExpanded: true,
      'onUpdate-provider': (_name: string, payload: any) => {
        updatedPayload = payload;
      },
    });
    app.mount(container);
    await nextTick();

    // 1. Click "配置端点" button
    const editBtn = container.querySelector('[data-testid="edit-protocols-btn"]') as HTMLButtonElement;
    expect(editBtn).not.toBeNull();
    editBtn.click();
    await nextTick();

    // 2. Endpoint inputs are now displayed
    const chatInput = container.querySelector('[data-testid="chat-url-input"]') as HTMLInputElement;
    expect(chatInput).not.toBeNull();

    // 3. Toggle Anthropic Messages pill
    const messagesPill = container.querySelector('[data-testid="protocol-pill-messages"]') as HTMLButtonElement;
    messagesPill.click();
    await nextTick();
    const messagesInput = container.querySelector('[data-testid="messages-url-input"]') as HTMLInputElement;
    expect(messagesInput).not.toBeNull();

    // 4. Fill endpoints
    chatInput.value = 'https://my-openai-proxy.com/v1';
    chatInput.dispatchEvent(new Event('input'));
    messagesInput.value = 'https://my-anthropic-proxy.com/v1';
    messagesInput.dispatchEvent(new Event('input'));

    // 5. Click save
    const saveBtn = container.querySelector('[data-testid="save-protocols-btn"]') as HTMLButtonElement;
    expect(saveBtn).not.toBeNull();
    saveBtn.click();
    await nextTick();

    // 6. Verify emitted payload
    expect(updatedPayload).not.toBeNull();
    expect(updatedPayload.chat_url).toBe('https://my-openai-proxy.com/v1');
    expect(updatedPayload.messages_url).toBe('https://my-anthropic-proxy.com/v1');
    expect(updatedPayload.responses_url).toBe('');

    app.unmount();
    document.body.removeChild(container);
  });
});
