import { ref, getCurrentInstance, onMounted } from 'vue';
import { adminApi } from '../lib/adminApi';
import { isPreconditionFailed } from '../lib/alova';
import type {
  GatewayKeyView,
  IssueGatewayKeyPayload,
  IssueGatewayKeyResponse,
} from '../types/admin';

/**
 * Scoped gateway credential management (task-28; design
 * `.agents/notes/web-users-design.md` F1–F7, API `.agents/notes/web-users-api.md`).
 *
 * Security notes baked in:
 * - The one-time plaintext lives in `issuedPlaintext` only; the view must clear
 *   it on modal close (`clearIssued`). It is NEVER persisted (no localStorage:
 *   the upstream-key quota persistence pattern must not be reused for secrets).
 * - `forbidden` (403) means "credential valid, scope too low" — surfaced as
 *   `canRead === false` so the view shows an empty state instead of bouncing
 *   the user to /connect (only 401 does that, via the session layer).
 */
export function useGatewayKeys(options: { autoFetch?: boolean } = {}) {
  const { autoFetch = true } = options;

  const gatewayKeys = ref<GatewayKeyView[]>([]);
  const configVersion = ref<number>(0);
  const loading = ref<boolean>(false);
  const error = ref<string | null>(null);
  /** Credential valid but lacking admin-read (inference login): show empty state. */
  const forbidden = ref<boolean>(false);
  const conflictDetected = ref<boolean>(false);
  /** One-time plaintext: cleared by `clearIssued()` when the modal closes. */
  const issuedPlaintext = ref<IssueGatewayKeyResponse | null>(null);

  function extractStatus(err: unknown): number | null {
    const anyErr = err as { response?: { status?: number }; status?: number };
    const status = anyErr?.response?.status ?? anyErr?.status;
    return typeof status === 'number' ? status : null;
  }

  async function fetchAll(): Promise<void> {
    loading.value = true;
    error.value = null;
    try {
      const rows = await adminApi.getGatewayKeys().send();
      gatewayKeys.value = rows;
      forbidden.value = false;
      if (rows.length > 0) {
        configVersion.value = rows[0].config_version;
      }
    } catch (err: unknown) {
      if (extractStatus(err) === 403) {
        // Not an error to shout about: this login simply has no admin-read.
        forbidden.value = true;
        gatewayKeys.value = [];
        error.value = null;
        return;
      }
      error.value = err instanceof Error ? err.message : String(err);
      throw err;
    } finally {
      loading.value = false;
    }
  }

  /** Silent refresh for background polling (no global loading mask). */
  async function refreshSilent(): Promise<void> {
    try {
      const rows = await adminApi.getGatewayKeys().send();
      gatewayKeys.value = rows;
      forbidden.value = false;
      if (rows.length > 0) {
        configVersion.value = rows[0].config_version;
      }
    } catch {
      // Transient network blips are ignored, matching useAdminConfig.
    }
  }

  async function runWithConflictCheck<T>(fn: () => Promise<T>): Promise<T> {
    try {
      return await fn();
    } catch (err: unknown) {
      if (!isPreconditionFailed(err)) throw err;
      try {
        await fetchAll();
      } catch {
        // Refresh failure must not block the retry; the second attempt decides.
      }
      try {
        return await fn();
      } catch (retryErr: unknown) {
        if (isPreconditionFailed(retryErr)) {
          conflictDetected.value = true;
        }
        throw retryErr;
      }
    }
  }

  async function issue(payload: IssueGatewayKeyPayload): Promise<IssueGatewayKeyResponse> {
    return runWithConflictCheck(async () => {
      const res = await adminApi.issueGatewayKey(payload, configVersion.value).send();
      issuedPlaintext.value = res;
      configVersion.value = res.config_version;
      await fetchAll();
      return res;
    });
  }

  async function revoke(id: string): Promise<GatewayKeyView> {
    return runWithConflictCheck(async () => {
      const res = await adminApi.revokeGatewayKey(id, configVersion.value).send();
      await fetchAll();
      return res;
    });
  }

  function clearIssued(): void {
    issuedPlaintext.value = null;
  }

  function clearConflict(): void {
    conflictDetected.value = false;
  }

  if (autoFetch && getCurrentInstance()) {
    onMounted(() => {
      void fetchAll().catch(() => {});
    });
  }

  return {
    gatewayKeys,
    configVersion,
    loading,
    error,
    forbidden,
    conflictDetected,
    issuedPlaintext,
    fetchAll,
    refreshSilent,
    issue,
    revoke,
    clearIssued,
    clearConflict,
  };
}
