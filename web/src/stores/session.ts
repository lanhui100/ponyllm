import { defineStore } from 'pinia';
import { ref } from 'vue';

export const SESSION_TOKEN_STORAGE_KEY = 'ponyllm_session_token';

function getInitialToken(): string {
  try {
    if (typeof window !== 'undefined' && window.sessionStorage) {
      const raw = window.sessionStorage.getItem(SESSION_TOKEN_STORAGE_KEY);
      if (typeof raw === 'string') {
        const trimmed = raw.trim();
        if (trimmed && trimmed !== 'null' && trimmed !== 'undefined') {
          return trimmed;
        }
      }
    }
  } catch {
    // Fallback if sessionStorage is inaccessible (e.g. strict security sandboxes)
  }
  return '';
}

function persistToken(nextToken: string): void {
  try {
    if (typeof window !== 'undefined' && window.sessionStorage) {
      // Always remove first to prevent stale token retention if setItem throws (quota/sandbox)
      window.sessionStorage.removeItem(SESSION_TOKEN_STORAGE_KEY);
      if (nextToken) {
        window.sessionStorage.setItem(SESSION_TOKEN_STORAGE_KEY, nextToken);
      }
    }
  } catch (err) {
    // Non-blocking fallback for quota exceeded or strict sandboxes
    console.warn('[PonyLLM] sessionStorage persistence unavailable, falling back to memory only:', err);
  }
}

// Session store with tab-scoped sessionStorage persistence.
// Maintains login across page reloads (F5) without leaking to persistent disk storage.
export const useSessionStore = defineStore('session', () => {
  const token = ref<string>(getInitialToken());
  // Single-flight flag: the first 401 owns the redirect; reset on login/logout
  // so the next session can redirect again (P0-2 hardening).
  const unauthorizedHandled = ref<boolean>(false);

  function login(nextToken: string): void {
    const trimmed = nextToken.trim();
    token.value = trimmed;
    unauthorizedHandled.value = false;
    persistToken(trimmed);
  }

  /// Explicit logout (login page): clears the token AND re-arms the flag so
  /// the next session can redirect on 401.
  function logout(): void {
    token.value = '';
    unauthorizedHandled.value = false;
    persistToken('');
  }

  /// Token wipe WITHOUT re-arming: used by the 401 single-flight path AFTER a
  /// successful claim. Re-arming here would let a concurrent second 401 claim
  /// the redirect again (P1-1: claim-then-wipe must not self-destruct).
  function clearToken(): void {
    token.value = '';
    persistToken('');
  }

  function markUnauthorizedHandled(): boolean {
    if (unauthorizedHandled.value) {
      return false;
    }
    unauthorizedHandled.value = true;
    return true;
  }

  return { token, unauthorizedHandled, login, logout, clearToken, markUnauthorizedHandled };
});

