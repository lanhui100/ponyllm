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

  it('renders refresh quota button and quota progress bars for antigravity provider keys', async () => {
    const container = document.createElement('div');
    document.body.appendChild(container);

    let testedKeyId: string | null = null;
    const antigravityProvider = {
      ...mockProvider,
      name: 'antigravity',
      default_protocol: 'antigravity',
    };
    const antigravityKeys = [
      {
        id: 'ag-key-1',
        provider: 'antigravity',
        masked_key: 'ya29.***',
        state: 'active' as const,
        priority: 1,
        weight: 10,
      },
    ];

    const mockKeyTestResults = {
      'ag-key-1': {
        success: true,
        latency_ms: 120,
        message: 'probe ok',
        quota_groups: [
          {
            display_name: 'Gemini Models',
            description: 'Gemini 2.5 & 3 series',
            buckets: [
              {
                bucket_id: 'gemini-5h',
                window: '5h',
                remaining_fraction: 0.85,
                reset_time_beijing: '2026-09-10 16:00:00',
                time_until_reset: '3小时40分后',
                display_name: '5小时用量',
                description: '5-hour quota',
              },
              {
                bucket_id: 'gemini-weekly',
                window: 'weekly',
                remaining_fraction: 0.62,
                reset_time_beijing: '2026-09-14 08:00:00',
                time_until_reset: '3天后',
                display_name: '周用量',
                description: 'Weekly quota',
              },
            ],
          },
        ],
      },
    };

    const app = createApp(ProviderCard, {
      provider: antigravityProvider,
      models: [],
      keys: antigravityKeys,
      adminWriteEnabled: true,
      keyTestResults: mockKeyTestResults,
      testingKeyIds: new Set<string>(),
      defaultExpanded: true,
      'onTest-single-key': (id: string) => {
        testedKeyId = id;
      },
    });
    app.mount(container);
    await nextTick();

    // Toggle keys to expand
    const toggleKeysBtn = container.querySelector('[data-testid="toggle-keys-btn"]') as HTMLButtonElement;
    expect(toggleKeysBtn).not.toBeNull();
    toggleKeysBtn.click();
    await nextTick();

    // Verify refresh button exists
    const refreshBtn = container.querySelector('[data-testid="refresh-key-quota-ag-key-1"]') as HTMLButtonElement;
    expect(refreshBtn).not.toBeNull();
    refreshBtn.click();
    await nextTick();
    expect(testedKeyId).toBe('ag-key-1');

    // Verify quota container and compact capsule progress bars
    const quotaContainer = container.querySelector('[data-testid="antigravity-quota-container"]');
    expect(quotaContainer).not.toBeNull();
    const geminiCapsule = container.querySelector('[data-testid="quota-capsule-gemini"]');
    expect(geminiCapsule).not.toBeNull();
    expect(geminiCapsule?.textContent).toContain('G');
    expect(geminiCapsule?.textContent).toContain('5h');
    expect(geminiCapsule?.textContent).toContain('85%');
    expect(geminiCapsule?.textContent).toContain('周');
    expect(geminiCapsule?.textContent).toContain('62%');

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
