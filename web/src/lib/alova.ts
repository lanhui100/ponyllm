import { createAlova } from 'alova';
import fetchAdapter from 'alova/fetch';
import type { Method } from 'alova';
import { useSessionStore } from '../stores/session';

// WEB-01 auth piece 2/3: the ONLY auth header the console ever sends.
// The gateway also accepts a secondary key header, but the console pins Bearer
// so the contract is single-headed and greppable (acceptance 6 asserts that
// secondary header string never appears under web/src).
export const AUTH_HEADER = 'Authorization';

export function bearerValue(rawToken: string): string {
  return `Bearer ${rawToken.trim()}`;
}

// 401 contract for all future callers (WEB-02+):
// - Pages never redirect/toast for 401 themselves (the single-flight handler
//   owns that); they MUST filter `UnauthorizedError` out before showing error
//   toasts, otherwise the user sees a duplicate "expired" toast.
// - `instanceof` check via `isUnauthorized`, never string matching on message.
export class UnauthorizedError extends Error {
  readonly status = 401;
  constructor() {
    super('unauthorized');
    this.name = 'UnauthorizedError';
  }
}

export function isUnauthorized(error: unknown): boolean {
  return error instanceof UnauthorizedError;
}

export class PreconditionFailedError extends Error {
  readonly status = 412;
  constructor(message = 'precondition_failed') {
    super(message);
    this.name = 'PreconditionFailedError';
  }
}

export function isPreconditionFailed(error: unknown): boolean {
  return error instanceof PreconditionFailedError;
}

export class AdminWriteDisabledError extends Error {
  readonly status = 404;
  constructor(message = 'admin_write_disabled') {
    super(message);
    this.name = 'AdminWriteDisabledError';
  }
}

export function isAdminWriteDisabled(error: unknown): boolean {
  return error instanceof AdminWriteDisabledError;
}

// 401 single-flight callback: set by the router so alova never imports vue-router
// (no cycle). First 401 claims it; later concurrent 401s are dropped.
let onFirstUnauthorized: (() => void) | null = null;

export function setUnauthorizedHandler(handler: () => void): void {
  onFirstUnauthorized = handler;
}

export function authHeaders(rawToken: string): Record<string, string> {
  const token = rawToken.trim();
  if (token === '') {
    return {};
  }
  return { [AUTH_HEADER]: bearerValue(token) };
}

// Runtime-configurable base URL (ADR: never baked as VITE_* build-time const).
// Same-origin relative by default; dev overrides via `window.__PONY_BASE__`.
// Normalized: trimmed, single trailing slash stripped (empty stays empty).
export function resolveBaseURL(): string {
  const override = (window as unknown as Record<string, unknown>).__PONY_BASE__;
  if (typeof override !== 'string') {
    return '';
  }
  return override.trim().replace(/\/+$/, '');
}

export const alova = createAlova({
  baseURL: resolveBaseURL(),
  requestAdapter: fetchAdapter(),
  beforeRequest(method: Method) {
    const session = useSessionStore();
    Object.assign(method.config.headers ??= {}, authHeaders(session.token));
  },
  responded: {
    async onSuccess(response: globalThis.Response, _method: Method) {
      if (response.status === 401) {
        handleUnauthorizedResponse();
        throw new UnauthorizedError();
      }
      if (response.status === 412) {
        throw new PreconditionFailedError();
      }
      if (!response.ok) {
        const errJson = await response.json().catch(() => ({}));
        if (response.status === 404 && errJson?.error?.code === 'admin_write_disabled') {
          throw new AdminWriteDisabledError();
        }
        throw new Error(errJson?.error?.message || `HTTP ${response.status}`);
      }
      // JSON-only success bodies in the M1 shell (documented assumption;
      // stream/non-JSON endpoints get their own alova instance in WEB-02+).
      return response.json();
    },
    onError(error: unknown, _method: Method) {
      throw error instanceof Error ? error : new Error(String(error));
    },
  },
});

// Extracted 401 path (unit-testable; P1-1 regression anchor).
// Claim-then-wipe WITHOUT re-arming: clearToken() keeps the single-flight flag
// so a concurrent second 401 cannot claim again. Re-arming happens only on
// explicit login()/logout().
export function handleUnauthorizedResponse(): boolean {
  const session = useSessionStore();
  if (session.markUnauthorizedHandled()) {
    session.clearToken();
    onFirstUnauthorized?.();
    return true;
  }
  return false;
}
