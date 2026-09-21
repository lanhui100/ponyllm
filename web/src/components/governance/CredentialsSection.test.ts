// @vitest-environment happy-dom
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { createApp, nextTick, defineComponent, h } from 'vue';
import CredentialsSection from './CredentialsSection.vue';
import { adminApi } from '../../lib/adminApi';
import type { GatewayKeyView } from '../../types/admin';

/**
 * CredentialsSection contract tests (hard-delete semantic since 2026-09-21):
 * - list renders prefix/last4 only (never a full key),
 * - inference scoped login (403 on list) shows the empty state, hides writes,
 * - issue shows the one-time plaintext and clears it on close,
 * - delete requires explicit confirmation and removes the row (no tombstone),
 * - writes are hidden when the admin write channel is closed.
 */

const rows: GatewayKeyView[] = [
  {
    id: 'agent-ci-1',
    scope: 'inference',
    prefix: 'sk-pony-infer-',
    last4: '9a70',
    revoked: false,
    expires_at: null,
    config_version: 7,
  },
  {
    id: 'old-viewer',
    scope: 'readonly',
    prefix: 'sk-pony-read-',
    last4: 'beef',
    revoked: false,
    expires_at: null,
    config_version: 7,
  },
];

function mountSection(props: { adminWriteEnabled: boolean; authCompat?: string }) {
  const container = document.createElement('div');
  document.body.appendChild(container);
  const Host = defineComponent({
    render: () => h(CredentialsSection, { ...props, onNotice: () => {} }),
  });
  const app = createApp(Host);
  app.mount(container);
  return { app, container };
}

function forbiddenError(): Error & { response: { status: number } } {
  const err = new Error('insufficient scope') as Error & { response: { status: number } };
  err.response = { status: 403 };
  return err;
}

describe('CredentialsSection (gateway scoped keys)', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });
  afterEach(() => {
    document.body.innerHTML = '';
  });

  it('renders identification fields only (no plaintext, no hash)', async () => {
    vi.spyOn(adminApi, 'getGatewayKeys').mockReturnValue({
      send: async () => rows,
    } as never);
    const { app, container } = mountSection({ adminWriteEnabled: true });
    await nextTick();
    await nextTick();

    const text = container.textContent || '';
    expect(text).toContain('agent-ci-1');
    expect(text).toContain('sk-pony-infer-');
    expect(text).toContain('9a70');
    expect(text).not.toContain('已吊销');
    expect(text).not.toContain('key_hash');
    app.unmount();
  });

  it('shows the forbidden empty state for an inference login (403 on list)', async () => {
    vi.spyOn(adminApi, 'getGatewayKeys').mockReturnValue({
      send: async () => {
        throw forbiddenError();
      },
    } as never);
    const { app, container } = mountSection({ adminWriteEnabled: true });
    await nextTick();
    await nextTick();

    expect(container.querySelector('[data-testid="credentials-forbidden"]')).not.toBeNull();
    // No write affordances for a credential that cannot even read.
    expect(container.querySelector('[data-testid="credentials-issue"]')).toBeNull();
    app.unmount();
  });

  it('shows one-time plaintext on issue and clears it when the modal closes', async () => {
    vi.spyOn(adminApi, 'getGatewayKeys').mockReturnValue({
      send: async () => rows,
    } as never);
    const issued = {
      id: 'new-agent',
      scope: 'inference',
      api_key: 'sk-pony-infer-0123456789abcdef0123456789abcdef',
      expires_at: null,
      config_version: 8,
    };
    vi.spyOn(adminApi, 'issueGatewayKey').mockReturnValue({
      send: async () => issued,
    } as never);

    const { app, container } = mountSection({ adminWriteEnabled: true });
    await nextTick();
    await nextTick();

    (container.querySelector('[data-testid="credentials-issue"]') as HTMLElement).click();
    await nextTick();
    const idInput = container.querySelector('[data-testid="credentials-issue-id"]') as HTMLInputElement;
    idInput.value = 'new-agent';
    idInput.dispatchEvent(new Event('input'));
    (container.querySelector('[data-testid="credentials-issue-submit"]') as HTMLElement).click();
    await nextTick();
    await nextTick();

    const secret = container.querySelector('[data-testid="credentials-secret-value"]');
    expect(secret).not.toBeNull();
    expect(secret!.textContent).toContain(issued.api_key);

    (container.querySelector('[data-testid="credentials-secret-close"]') as HTMLElement).click();
    await nextTick();
    expect(container.querySelector('[data-testid="credentials-secret-value"]')).toBeNull();
    app.unmount();
  });

  it('requires confirmation before deleting, then removes the row', async () => {
    const listMock = vi.spyOn(adminApi, 'getGatewayKeys').mockReturnValue({
      send: async () => rows,
    } as never);
    const revokeSpy = vi.spyOn(adminApi, 'revokeGatewayKey').mockReturnValue({
      send: async () => ({ ...rows[0] }),
    } as never);

    const { app, container } = mountSection({ adminWriteEnabled: true });
    await nextTick();
    await nextTick();

    (container.querySelector('[data-testid="credentials-revoke-agent-ci-1"]') as HTMLElement).click();
    await nextTick();
    // Confirmation modal appears and nothing has been sent yet.
    expect(container.querySelector('[data-testid="credentials-revoke-modal"]')).not.toBeNull();
    expect(revokeSpy).not.toHaveBeenCalled();

    // After a real hard delete the follow-up list no longer contains the row.
    listMock.mockReturnValue({
      send: async () => rows.filter((r) => r.id !== 'agent-ci-1'),
    } as never);
    (container.querySelector('[data-testid="credentials-revoke-confirm"]') as HTMLElement).click();
    // Flush the full async chain: revoke POST -> refetch list -> local filter -> re-render.
    await new Promise((r) => setTimeout(r, 20));
    await nextTick();
    await nextTick();
    expect(revokeSpy).toHaveBeenCalledWith('agent-ci-1', 7);
    // Hard delete: the row vanishes entirely (no "已吊销" tombstone).
    expect(container.querySelector('[data-testid="credentials-row-agent-ci-1"]')).toBeNull();
    expect(container.querySelector('[data-testid="credentials-revoke-agent-ci-1"]')).toBeNull();
    app.unmount();
  });

  it('hides write actions when the admin write channel is closed', async () => {
    vi.spyOn(adminApi, 'getGatewayKeys').mockReturnValue({
      send: async () => rows,
    } as never);
    const { app, container } = mountSection({ adminWriteEnabled: false });
    await nextTick();
    await nextTick();

    expect(container.querySelector('[data-testid="credentials-issue"]')).toBeNull();
    expect(container.querySelector('[data-testid="credentials-revoke-agent-ci-1"]')).toBeNull();
    app.unmount();
  });

  it('warns in the banner when auth_compat is strict', async () => {
    vi.spyOn(adminApi, 'getGatewayKeys').mockReturnValue({
      send: async () => rows,
    } as never);
    const { app, container } = mountSection({ adminWriteEnabled: true, authCompat: 'strict' });
    await nextTick();
    await nextTick();
    expect(container.textContent).toContain('strict');
    expect(container.textContent).toContain('旧版单 token 全部 401');
    app.unmount();
  });
});

describe('CredentialsSection loading state', () => {
  afterEach(() => {
    document.body.innerHTML = '';
  });

  it('shows a loading hint instead of an empty table on first paint', async () => {
    let release: (v: GatewayKeyView[]) => void = () => {};
    vi.spyOn(adminApi, 'getGatewayKeys').mockReturnValue({
      send: () =>
        new Promise<GatewayKeyView[]>((resolve) => {
          release = resolve;
        }),
    } as never);

    const { app, container } = mountSection({ adminWriteEnabled: true });
    await nextTick();
    // Nothing resolved yet: no zero-row table masquerading as "no credentials".
    expect(container.querySelector('[data-testid="credentials-loading"]')).not.toBeNull();
    expect(container.querySelector('[data-testid="credentials-table"]')).toBeNull();

    release([]);
    await nextTick();
    await nextTick();
    expect(container.querySelector('[data-testid="credentials-loading"]')).toBeNull();
    app.unmount();
  });
});

describe('CredentialsSection scope display labels', () => {
  afterEach(() => {
    document.body.innerHTML = '';
  });

  it('shows friendly Chinese scope names and never the raw scope enum', async () => {
    vi.spyOn(adminApi, 'getGatewayKeys').mockReturnValue({
      send: async () => [
        { id: 'a1', scope: 'admin', prefix: 'sk-pony-admin-', last4: '1111', revoked: false, expires_at: null, config_version: 7 },
        { id: 'i1', scope: 'inference', prefix: 'sk-pony-infer-', last4: '2222', revoked: false, expires_at: null, config_version: 7 },
        { id: 'r1', scope: 'readonly', prefix: 'sk-pony-read-', last4: '3333', revoked: false, expires_at: null, config_version: 7 },
      ],
    } as never);
    const { app, container } = mountSection({ adminWriteEnabled: true });
    await nextTick();
    await nextTick();
    await new Promise((r) => setTimeout(r, 20));
    const table = container.querySelector('[data-testid="credentials-table"]');
    expect(table).not.toBeNull();
    const text = table!.textContent || '';
    // Friendly names shown…
    expect(text).toContain('管理员');
    expect(text).toContain('调用方');
    expect(text).toContain('只读');
    // …and the raw API contract values never leak into badge/labels.
    for (const badge of Array.from(table!.querySelectorAll('.inline-flex'))) {
      expect(['管理员', '调用方', '只读']).toContain((badge.textContent || '').trim());
    }
    app.unmount();
  });
});
