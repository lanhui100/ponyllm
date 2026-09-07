import { defineStore } from 'pinia';
import { ref } from 'vue';

// WEB-01 auth piece 1/3: token lives ONLY in memory (Pinia state).
// Zero browser persistence of any kind, ever (acceptance 6 greps web/src
// for persistence identifiers; any hit fails the gate).
export const useSessionStore = defineStore('session', () => {
  const token = ref<string>('');
  // Single-flight flag: the first 401 owns the redirect; reset on login/logout
  // so the next session can redirect again (P0-2 hardening).
  const unauthorizedHandled = ref<boolean>(false);

  function login(nextToken: string): void {
    token.value = nextToken.trim();
    unauthorizedHandled.value = false;
  }

  /// Explicit logout (login page): clears the token AND re-arms the flag so
  /// the next session can redirect on 401.
  function logout(): void {
    token.value = '';
    unauthorizedHandled.value = false;
  }

  /// Token wipe WITHOUT re-arming: used by the 401 single-flight path AFTER a
  /// successful claim. Re-arming here would let a concurrent second 401 claim
  /// the redirect again (P1-1: claim-then-wipe must not self-destruct).
  function clearToken(): void {
    token.value = '';
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
