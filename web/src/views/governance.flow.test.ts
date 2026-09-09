// @vitest-environment happy-dom
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { createApp, nextTick } from 'vue';
import { createPinia, setActivePinia } from 'pinia';
import { createRouter, createMemoryHistory } from 'vue-router';
import GovernanceView from './GovernanceView.vue';
import { useSessionStore } from '../stores/session';
import { adminApi } from '../lib/adminApi';
import { PreconditionFailedError } from '../lib/alova';
import type {
  OverviewView,
  ProviderView,
  ModelView,
  KeyView,
  StrategyView,
  CreateKeyResponse,
} from '../types/admin';

describe('GovernanceView End-to-End User Flow (WEB-04)', () => {
  let router: ReturnType<typeof createRouter>;
  let pinia: ReturnType<typeof createPinia>;
  let container: HTMLDivElement;

  const mockOverviewReadonly: OverviewView = {
    version: '0.2.26',
    bind: '127.0.0.1:8080',
    auth_mode: 'token',
    providers: 1,
    keys: 1,
    keys_active: 1,
    strategy: 'economy',
    hot_reload_ms: 1000,
    admin_write_enabled: false,
    config_version: 10,
  };

  const mockOverviewWritable: OverviewView = {
    ...mockOverviewReadonly,
    admin_write_enabled: true,
  };

  const mockProviders: ProviderView[] = [
    {
      name: 'openai',
      base_url: 'https://api.openai.com/v1',
      default_model: 'gpt-4o',
      strategy: 'economy',
      billing_mode: 'token',
      input_price: 2.5,
      cached_price: 1.25,
      output_price: 10.0,
      models: 1,
    },
  ];

  const mockModels: ModelView[] = [
    {
      name: 'gpt-4o',
      tier: 'Smart',
      context_window: '128k',
      thinking_default: 'Off',
      thinking_max: 'High',
    },
  ];

  const mockKeys: KeyView[] = [
    {
      id: 'key-01',
      provider: 'openai',
      masked_key: 'sk-proj-****',
      priority: 1,
      weight: 10,
      state: 'active',
    },
  ];

  const mockStrategy: StrategyView = {
    strategy: 'economy',
    config_version: 10,
  };

  beforeEach(() => {
    window.sessionStorage?.clear();
    pinia = createPinia();
    setActivePinia(pinia);
    router = createRouter({
      history: createMemoryHistory(),
      routes: [
        { path: '/', redirect: '/governance' },
        { path: '/dashboard', component: { template: '<div>dashboard</div>' } },
        { path: '/recorder', component: { template: '<div>recorder</div>' } },
        { path: '/governance', component: GovernanceView },
      ],
    });
    container = document.createElement('div');
    document.body.appendChild(container);
  });

  afterEach(() => {
    vi.restoreAllMocks();
    document.body.removeChild(container);
  });

  it('Flow 1: Readonly mode renders readonly banner and disables CUD buttons', async () => {
    const session = useSessionStore(pinia);
    session.login('sk-admin-token');

    vi.spyOn(adminApi, 'getOverview').mockReturnValue({ send: () => Promise.resolve(mockOverviewReadonly) } as any);
    vi.spyOn(adminApi, 'getProviders').mockReturnValue({ send: () => Promise.resolve(mockProviders) } as any);
    vi.spyOn(adminApi, 'getModels').mockReturnValue({ send: () => Promise.resolve(mockModels) } as any);
    vi.spyOn(adminApi, 'getKeys').mockReturnValue({ send: () => Promise.resolve(mockKeys) } as any);
    vi.spyOn(adminApi, 'getStrategy').mockReturnValue({ send: () => Promise.resolve(mockStrategy) } as any);

    const app = createApp(GovernanceView);
    app.use(router);
    app.use(pinia);
    app.mount(container);

    await nextTick();
    await new Promise((r) => setTimeout(r, 10));

    // Assert readonly banner exists
    const banner = container.querySelector('[data-testid="readonly-banner"]');
    expect(banner).not.toBeNull();
    expect(banner?.textContent).toContain('只读治理模式');

    // Assert Add Provider button is disabled
    const addProviderBtn = container.querySelector('[data-testid="add-provider-btn"]') as HTMLButtonElement;
    expect(addProviderBtn).not.toBeNull();
    expect(addProviderBtn.disabled).toBe(true);

    // Verify only providers and strategy tabs exist (models and keys tabs cleaned up)
    expect(container.querySelector('[data-testid="tab-providers"]')).not.toBeNull();
    expect(container.querySelector('[data-testid="tab-strategy"]')).not.toBeNull();
    expect(container.querySelector('[data-testid="tab-models"]')).toBeNull();
    expect(container.querySelector('[data-testid="tab-keys"]')).toBeNull();

    // In readonly mode, key deletion and dial-test buttons in ProviderCard are disabled
    const deleteKeyBtn = container.querySelector('[data-testid="delete-key-btn"]') as HTMLButtonElement;
    if (deleteKeyBtn) {
      expect(deleteKeyBtn.disabled).toBe(true);
    }
  });

  it('Flow 2: Key creation displays one-time plaintext modal and destroys it on close', async () => {
    const session = useSessionStore(pinia);
    session.login('sk-admin-token');

    vi.spyOn(adminApi, 'getOverview').mockReturnValue({ send: () => Promise.resolve(mockOverviewWritable) } as any);
    vi.spyOn(adminApi, 'getProviders').mockReturnValue({ send: () => Promise.resolve(mockProviders) } as any);
    vi.spyOn(adminApi, 'getModels').mockReturnValue({ send: () => Promise.resolve(mockModels) } as any);
    vi.spyOn(adminApi, 'getKeys').mockReturnValue({ send: () => Promise.resolve(mockKeys) } as any);
    vi.spyOn(adminApi, 'getStrategy').mockReturnValue({ send: () => Promise.resolve(mockStrategy) } as any);

    const mockCreateKeyResp: CreateKeyResponse = {
      id: 'key-new',
      provider: 'openai',
      api_key: 'sk-plaintext-secret-test-value',
      priority: 1,
      weight: 10,
      state: 'active',
      config_version: 11,
    };

    vi.spyOn(adminApi, 'createKey').mockReturnValue({ send: () => Promise.resolve(mockCreateKeyResp) } as any);

    const app = createApp(GovernanceView);
    app.use(router);
    app.use(pinia);
    app.mount(container);

    await nextTick();
    await new Promise((r) => setTimeout(r, 10));

    // In all-in-one provider view, click Add Key button on ProviderCard
    const addKeyBtn = container.querySelector('[data-testid="add-key-btn"]') as HTMLButtonElement;
    expect(addKeyBtn).not.toBeNull();
    expect(addKeyBtn.disabled).toBe(false);
    addKeyBtn.click();
    await nextTick();

    const idInput = container.querySelector('[data-testid="key-id-input"]') as HTMLInputElement;
    const secretInput = container.querySelector('[data-testid="key-secret-input"]') as HTMLInputElement;
    expect(idInput).not.toBeNull();
    expect(secretInput).not.toBeNull();
    idInput.value = 'key-new';
    idInput.dispatchEvent(new Event('input'));
    secretInput.value = 'sk-plaintext-secret-test-value';
    secretInput.dispatchEvent(new Event('input'));

    const submitBtn = container.querySelector('[data-testid="submit-key-btn"]') as HTMLButtonElement;
    submitBtn.click();

    await nextTick();
    await new Promise((r) => setTimeout(r, 20));

    // KeySecretModal must appear with plaintext key
    const plaintextInput = container.querySelector('[data-testid="plaintext-key-input"]') as HTMLInputElement;
    expect(plaintextInput).not.toBeNull();
    expect(plaintextInput.value).toBe('sk-plaintext-secret-test-value');

    // Close modal
    const closeModalBtn = container.querySelector('[data-testid="close-key-modal-btn"]') as HTMLButtonElement;
    expect(closeModalBtn).not.toBeNull();
    closeModalBtn.click();

    await nextTick();
    // Modal is gone, plaintext input no longer in DOM
    expect(container.querySelector('[data-testid="plaintext-key-input"]')).toBeNull();
  });

  it('Flow 3: 412 Conflict modal pops up on precondition failure and reloads on refresh', async () => {
    const session = useSessionStore(pinia);
    session.login('sk-admin-token');

    vi.spyOn(adminApi, 'getOverview').mockReturnValue({ send: () => Promise.resolve(mockOverviewWritable) } as any);
    vi.spyOn(adminApi, 'getProviders').mockReturnValue({ send: () => Promise.resolve(mockProviders) } as any);
    vi.spyOn(adminApi, 'getModels').mockReturnValue({ send: () => Promise.resolve(mockModels) } as any);
    vi.spyOn(adminApi, 'getKeys').mockReturnValue({ send: () => Promise.resolve(mockKeys) } as any);
    vi.spyOn(adminApi, 'getStrategy').mockReturnValue({ send: () => Promise.resolve(mockStrategy) } as any);

    vi.spyOn(adminApi, 'createProvider').mockReturnValue({
      send: () => Promise.reject(new PreconditionFailedError()),
    } as any);

    const app = createApp(GovernanceView);
    app.use(router);
    app.use(pinia);
    app.mount(container);

    await nextTick();
    await new Promise((r) => setTimeout(r, 10));

    // Open add provider drawer
    const addProviderBtn = container.querySelector('[data-testid="add-provider-btn"]') as HTMLButtonElement;
    addProviderBtn.click();
    await nextTick();

    const nameInput = container.querySelector('[data-testid="provider-name-input"]') as HTMLInputElement;
    const urlInput = container.querySelector('[data-testid="provider-base-url-input"]') as HTMLInputElement;
    nameInput.value = 'deepseek';
    nameInput.dispatchEvent(new Event('input'));
    urlInput.value = 'https://api.deepseek.com/v1';
    urlInput.dispatchEvent(new Event('input'));

    const submitBtn = container.querySelector('[data-testid="submit-provider-btn"]') as HTMLButtonElement;
    submitBtn.click();

    await nextTick();
    await new Promise((r) => setTimeout(r, 20));

    // ConflictModal must be visible
    const refreshBtn = container.querySelector('[data-testid="refresh-config-btn"]') as HTMLButtonElement;
    expect(refreshBtn).not.toBeNull();

    // Click refresh
    refreshBtn.click();
    await nextTick();
    await new Promise((r) => setTimeout(r, 10));

    // Conflict modal is dismissed
    expect(container.querySelector('[data-testid="refresh-config-btn"]')).toBeNull();
  });

  it('Flow 4: Antigravity provider creation via OAuth flow', async () => {
    vi.spyOn(adminApi, 'getOverview').mockReturnValue({
      send: () => Promise.resolve(mockOverviewWritable),
    } as any);
    vi.spyOn(adminApi, 'getProviders').mockReturnValue({
      send: () => Promise.resolve(mockProviders),
    } as any);
    vi.spyOn(adminApi, 'getModels').mockReturnValue({
      send: () => Promise.resolve(mockModels),
    } as any);
    vi.spyOn(adminApi, 'getKeys').mockReturnValue({
      send: () => Promise.resolve(mockKeys),
    } as any);
    vi.spyOn(adminApi, 'getStrategy').mockReturnValue({
      send: () => Promise.resolve(mockStrategy),
    } as any);

    const mockAuthUrl = {
      auth_url: 'https://accounts.google.com/o/oauth2/v2/auth?client_id=dummy',
      redirect_uri: 'http://localhost:51121/oauth2callback',
      state: 'state-123',
    };
    const getAuthUrlSpy = vi.spyOn(adminApi, 'getAntigravityAuthUrl').mockReturnValue({
      send: () => Promise.resolve(mockAuthUrl),
    } as any);

    const authorizeSpy = vi.spyOn(adminApi, 'authorizeAntigravity').mockReturnValue({
      send: () => Promise.resolve({
        provider: 'antigravity',
        id: 'ag-test@gmail.com',
        email: 'test@gmail.com',
        config_version: 11,
      }),
    } as any);

    const app = createApp(GovernanceView);
    app.use(router);
    app.use(pinia);
    app.mount(container);

    await nextTick();
    await new Promise((r) => setTimeout(r, 10));

    // 1. Click add provider button
    const addProviderBtn = container.querySelector('[data-testid="add-provider-btn"]') as HTMLButtonElement;
    addProviderBtn.click();
    await nextTick();

    // 2. Switch to Antigravity mode
    const agModeBtn = container.querySelector('[data-testid="mode-antigravity-btn"]') as HTMLButtonElement;
    expect(agModeBtn).not.toBeNull();
    agModeBtn.click();
    await nextTick();
    await new Promise((r) => setTimeout(r, 10));

    expect(getAuthUrlSpy).toHaveBeenCalled();

    // 3. Antigravity inputs and smart proxy capsule are rendered
    const proxyCapsule = container.querySelector('[data-testid="proxy-status-capsule"]');
    expect(proxyCapsule).not.toBeNull();

    const codeInput = container.querySelector('[data-testid="ag-code-input"]') as HTMLInputElement;
    expect(codeInput).not.toBeNull();

    // 4. Fill in authorization code / URL
    codeInput.value = 'http://localhost:51121/oauth2callback?code=mock-code-123';
    codeInput.dispatchEvent(new Event('input'));
    await nextTick();

    // 5. Submit Antigravity authorization
    const submitAgBtn = container.querySelector('[data-testid="submit-ag-provider-btn"]') as HTMLButtonElement;
    expect(submitAgBtn).not.toBeNull();
    submitAgBtn.click();
    await nextTick();
    await new Promise((r) => setTimeout(r, 20));

    expect(authorizeSpy).toHaveBeenCalledWith(expect.objectContaining({
      code_or_url: 'http://localhost:51121/oauth2callback?code=mock-code-123',
      provider: 'antigravity',
    }));
  });

  it('Flow 5: Antigravity automated OAuth postMessage callback seamlessly completes authorization', async () => {
    vi.spyOn(adminApi, 'getOverview').mockReturnValue({
      send: () => Promise.resolve(mockOverviewWritable),
    } as any);
    vi.spyOn(adminApi, 'getProviders').mockReturnValue({
      send: () => Promise.resolve(mockProviders),
    } as any);
    vi.spyOn(adminApi, 'getModels').mockReturnValue({
      send: () => Promise.resolve(mockModels),
    } as any);
    vi.spyOn(adminApi, 'getKeys').mockReturnValue({
      send: () => Promise.resolve(mockKeys),
    } as any);
    vi.spyOn(adminApi, 'getStrategy').mockReturnValue({
      send: () => Promise.resolve(mockStrategy),
    } as any);
    vi.spyOn(adminApi, 'getProxyStatus').mockReturnValue({
      send: () => Promise.resolve({
        available: true,
        proxy_url: 'http://127.0.0.1:8899',
        proxy_type: 'pproxy',
        description: '本地 pproxy 智能出海代理 (127.0.0.1:8899) 运行中',
        latency_ms: 120,
        hint: '已自动接管',
      }),
    } as any);

    vi.spyOn(adminApi, 'getAntigravityAuthUrl').mockReturnValue({
      send: () => Promise.resolve({
        auth_url: 'https://accounts.google.com/o/oauth2/v2/auth?mock=true',
        redirect_uri: 'http://localhost:8080/oauth2callback',
        state: 'auto-test-state-999',
      }),
    } as any);

    const authorizeSpy = vi.spyOn(adminApi, 'authorizeAntigravity').mockReturnValue({
      send: () => Promise.resolve({
        provider: 'antigravity',
        id: 'ag-seamless@gmail.com',
        email: 'seamless@gmail.com',
        config_version: 12,
      }),
    } as any);

    // Mock window.open
    const openSpy = vi.spyOn(window, 'open').mockImplementation(() => null);

    const app = createApp(GovernanceView);
    app.use(router);
    app.use(pinia);
    app.mount(container);

    await nextTick();
    await new Promise((r) => setTimeout(r, 10));

    // Open add provider and switch to Antigravity
    const addProviderBtn = container.querySelector('[data-testid="add-provider-btn"]') as HTMLButtonElement;
    addProviderBtn.click();
    await nextTick();

    const agModeBtn = container.querySelector('[data-testid="mode-antigravity-btn"]') as HTMLButtonElement;
    agModeBtn.click();
    await nextTick();
    await new Promise((r) => setTimeout(r, 10));

    // Click "前往 Google 授权"
    const fetchUrlBtn = container.querySelector('[data-testid="ag-fetch-url-btn"]') as HTMLButtonElement;
    fetchUrlBtn.click();
    await nextTick();
    await new Promise((r) => setTimeout(r, 10));

    expect(openSpy).toHaveBeenCalled();
    // Waiting indicator is visible
    expect(container.querySelector('[data-testid="ag-waiting-indicator"]')).not.toBeNull();

    // Simulate malicious cross-origin postMessage (attack attempt)
    window.dispatchEvent(
      new MessageEvent('message', {
        origin: 'https://malicious-attacker.com',
        data: {
          type: 'antigravity:oauth_callback',
          success: true,
          code: '4/0A-stolen-code',
          state: 'auto-test-state-999',
        },
      })
    );
    await nextTick();
    expect(authorizeSpy).not.toHaveBeenCalled();

    // Simulate state-mismatch postMessage (CSRF attempt)
    window.dispatchEvent(
      new MessageEvent('message', {
        origin: window.location.origin,
        data: {
          type: 'antigravity:oauth_callback',
          success: true,
          code: '4/0A-csrf-code',
          state: 'wrong-state-xyz',
        },
      })
    );
    await nextTick();
    expect(authorizeSpy).not.toHaveBeenCalled();

    // Simulate legitimate OAuth callback window sending postMessage with matching origin and state
    window.dispatchEvent(
      new MessageEvent('message', {
        origin: window.location.origin,
        data: {
          type: 'antigravity:oauth_callback',
          success: true,
          code: '4/0A-postmessage-seamless-code',
          state: 'auto-test-state-999',
        },
      })
    );

    await nextTick();
    await new Promise((r) => setTimeout(r, 20));

    // Verify authorizeAntigravity was automatically triggered without manual user input!
    expect(authorizeSpy).toHaveBeenCalledWith(expect.objectContaining({
      code_or_url: '4/0A-postmessage-seamless-code',
      provider: 'antigravity',
      state: 'auto-test-state-999',
    }));

    openSpy.mockRestore();

  });
});
