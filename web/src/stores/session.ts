import { defineStore } from 'pinia';
import { ref } from 'vue';

// VULN-05 (Phase-3): Bearer token 不再经 sessionStorage 持久化。
// 该键常量保留导出仅作验收断言锚点（"永不读写"），业务代码不得读写。
export const SESSION_TOKEN_STORAGE_KEY = 'ponyllm_session_token';
// 发版强制下线版本钉：非敏感 UI 状态，按契约保留 sessionStorage。
export const SESSION_GATEWAY_VERSION_KEY = 'ponyllm_gateway_version';

function readStoredGatewayVersion(): string {
  try {
    if (typeof window !== 'undefined' && window.sessionStorage) {
      const raw = window.sessionStorage.getItem(SESSION_GATEWAY_VERSION_KEY);
      if (typeof raw === 'string' && raw.trim() !== '') {
        return raw.trim();
      }
    }
  } catch {
    // Sandboxed storage: treat as first-seen (no forced logout).
  }
  return '';
}

function writeStoredGatewayVersion(version: string): void {
  try {
    if (typeof window !== 'undefined' && window.sessionStorage) {
      window.sessionStorage.setItem(SESSION_GATEWAY_VERSION_KEY, version);
    }
  } catch {
    // Non-blocking: version pin is best-effort.
  }
}

// Session store（VULN-05 重构）：
// - cookie 模式（会话端点启用）：凭据由 HttpOnly cookie 接管，token 恒空，
//   `loggedIn` 为内存"已登录"态；
// - legacy 模式（会话端点未启用，404 回退）：token 仅存内存 ref（旧部署
//   向后兼容，见 negotiateSessionMode）。
// 无论哪种模式，会话端点启用时 XSS 可读存储不再承载凭据。
export const useSessionStore = defineStore('session', () => {
  const token = ref<string>('');
  const loggedIn = ref<boolean>(false);
  const sessionMode = ref<'unknown' | 'cookie' | 'legacy'>('unknown');
  // R-S4 (Phase-3b): CSRF 双提交通道 —— cookie 会话 sid 仅存内存（来自换发/探活
  // 响应体 body.sid，后端 R-S8），用于写请求 X-Pony-Session 头；不落任何 storage。
  const sid = ref<string | null>(null);
  // Single-flight flag: the first 401 owns the redirect; reset on login/logout
  // so the next session can redirect again (P0-2 hardening).
  const unauthorizedHandled = ref<boolean>(false);

  /// 统一会话判定（router 守卫 / alova / telemetry 共用；替代旧 `token !== ''`）。
  function hasSession(): boolean {
    return sessionMode.value === 'cookie' ? loggedIn.value : token.value !== '';
  }

  /// legacy 模式登录：内存 token。
  function login(nextToken: string): void {
    token.value = nextToken.trim();
    loggedIn.value = true;
    unauthorizedHandled.value = false;
  }

  /// cookie 模式登录成功：token 丢弃（HttpOnly cookie 接管），仅记"已登录"；
  /// `sessionSid` 来自换发响应体 body.sid（R-S4 内存 CSRF 通道）。
  function loginCookieMode(sessionSid?: string | null): void {
    token.value = '';
    loggedIn.value = true;
    sessionMode.value = 'cookie';
    sid.value = typeof sessionSid === 'string' && sessionSid.trim() !== '' ? sessionSid.trim() : null;
    unauthorizedHandled.value = false;
  }

  /// Explicit logout：cookie 模式先 fire-and-forget 服务端吊销（best-effort，
  /// 失败/404 不影响清态），再清内存态并重挂单飞旗标。
  function logout(): void {
    if (sessionMode.value === 'cookie') {
      void fetch('/api/admin/session/revoke', {
        method: 'POST',
        headers: sid.value ? { 'X-Pony-Session': sid.value } : {},
        credentials: 'same-origin',
      }).catch(() => {
        // best-effort: 吊销失败（网络/端点禁用）不阻塞本地清态
      });
    }
    token.value = '';
    loggedIn.value = false;
    sid.value = null;
    unauthorizedHandled.value = false;
  }

  /// P2: release-forced logout. Compares the gateway `/health` version against
  /// the last-seen version in sessionStorage; on mismatch the stored session
  /// is wiped (old keys/sessions must not survive a release) and the caller
  /// shows a "re-connect" notice. Returns true when a wipe happened.
  /// Pure of network: the caller fetches `/health` and passes `version` in.
  function logoutIfGatewayUpgraded(gatewayVersion: string): boolean {
    const seen = readStoredGatewayVersion();
    writeStoredGatewayVersion(gatewayVersion);
    if (seen !== '' && seen !== gatewayVersion) {
      logout();
      return true;
    }
    return false;
  }

  /// Token/session wipe WITHOUT re-arming: used by the 401 single-flight path
  /// AFTER a successful claim (P1-1: claim-then-wipe must not self-destruct).
  function clearToken(): void {
    token.value = '';
    loggedIn.value = false;
    sid.value = null;
  }

  function markUnauthorizedHandled(): boolean {
    if (unauthorizedHandled.value) {
      return false;
    }
    unauthorizedHandled.value = true;
    return true;
  }

  /// 会话端点协商（幂等缓存，router 守卫首次导航触发）：
  /// - GET /api/admin/session 200  → cookie 模式 + 已登录（并存响应体 sid 至内存）
  /// - 401（含 session_expired 信封）→ cookie 模式 + 未登录
  /// - 404（端点未启用）→ legacy 回退（向后兼容旧部署）
  /// - 其它/网络错误 → 保持 unknown（401 统一路径兜底）
  async function negotiateSessionMode(): Promise<'unknown' | 'cookie' | 'legacy'> {
    if (sessionMode.value !== 'unknown') {
      return sessionMode.value;
    }
    try {
      const resp = await fetch('/api/admin/session', {
        method: 'GET',
        credentials: 'same-origin',
      });
      if (resp.status === 200) {
        const body = (await resp.json().catch(() => ({}))) as { sid?: unknown; authenticated?: unknown };
        sessionMode.value = 'cookie';
        loggedIn.value = body?.authenticated !== false;
        sid.value =
          typeof body?.sid === 'string' && body.sid.trim() !== '' ? body.sid.trim() : null;
      } else if (resp.status === 401) {
        sessionMode.value = 'cookie';
        loggedIn.value = false;
        sid.value = null;
      } else if (resp.status === 404) {
        sessionMode.value = 'legacy';
        sid.value = null;
      }
    } catch {
      // 网关不可达：保持 unknown，后续 401 统一路径兜底。
    }
    return sessionMode.value;
  }

  return {
    token,
    loggedIn,
    sessionMode,
    sid,
    hasSession,
    login,
    loginCookieMode,
    logout,
    logoutIfGatewayUpgraded,
    clearToken,
    markUnauthorizedHandled,
    negotiateSessionMode,
  };
});
