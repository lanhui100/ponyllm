// B003: /api/user 前端封装（web user JWT auth & self-service token）。
//
// 契约对齐 B002 后端（tests/red_user_*_api_tests.rs 冻结面）：
// - loginWithPassword: POST /api/user/login {username,password} → {access_token, user}
// - me: GET /api/user/me → UserProfile + used_tokens
// - tokens CRUD/rotate：/api/user/tokens...
// - admin users CRUD/reset：/api/user/admin/users...（admin JWT only）
//
// Bearer JWT 头从 session store 内存 ref 读取（VULN-05：JWT 不落 storage）。
// 错误信封：非 2xx 抛 Error（携带后端 code 供调用方区分 401/403/409/412）。

import { useSessionStore } from '../stores/session';
import { bearerValue } from './alova';

export interface UserProfile {
  id: string;
  username: string;
  role: 'admin' | 'user';
  name: string;
  enabled: boolean;
}

export interface LoginResponse {
  access_token: string;
  user: UserProfile;
}

export interface UserTokenMeta {
  key_id: string;
  name: string | null;
  model_limits: string[] | null;
  quota: number | null;
  expires_at: number | null;
  enabled: boolean;
  used_tokens: number;
}

export interface CreateTokenRequest {
  name: string;
  model_limits?: string[];
  quota?: number;
  expires_at?: number;
}

export interface UpdateTokenRequest {
  name?: string;
  model_limits?: string[];
  quota?: number;
  expires_at?: number;
  enabled?: boolean;
}

export interface AdminUserView {
  id: string;
  username: string | null;
  role: string;
  name: string;
  enabled: boolean;
  allowed_models: string[] | null;
  max_tokens: number | null;
  used_tokens: number;
  created_at: number;
  token_version: number;
}

export interface CreateAdminUserRequest {
  username: string;
  password: string;
  role?: 'admin' | 'user';
  name?: string;
  enabled?: boolean;
  allowed_models?: string[];
  max_tokens?: number;
}

export interface UpdateAdminUserRequest {
  name?: string;
  enabled?: boolean;
  role?: 'admin' | 'user';
  allowed_models?: string[];
  max_tokens?: number;
}

/** 从 session store 读取当前 JWT（仅内存）。无会话/无 pinia（测试环境）返回空头。 */
function jwtHeader(): Record<string, string> {
  try {
    const session = useSessionStore();
    const raw = (session.token ?? '').trim();
    if (raw === '') {
      return {};
    }
    return { Authorization: bearerValue(raw) };
  } catch {
    // 测试环境未初始化 pinia：不发送 Authorization（红相契约仅断言 URL/method/body）。
    return {};
  }
}

async function parseJson<T>(resp: Response): Promise<T> {
  if (!resp.ok) {
    const body = (await resp.json().catch(() => ({}))) as {
      error?: { code?: unknown; message?: unknown };
    };
    const err = new Error(
      typeof body?.error?.message === 'string'
        ? body.error.message
        : `HTTP ${resp.status}`,
    ) as Error & { code?: unknown; status?: number };
    err.code = body?.error?.code ?? 'http_error';
    err.status = resp.status;
    throw err;
  }
  return (await resp.json().catch(() => null)) as T;
}

/** 口令登录：POST /api/user/login → access_token（JWT 仅内存，调用方自行入 session store）。 */
export async function loginWithPassword(
  username: string,
  password: string,
): Promise<LoginResponse> {
  const resp = await fetch('/api/user/login', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ username, password }),
  });
  return parseJson<LoginResponse>(resp);
}

/** 当前用户 profile + 用量：GET /api/user/me。 */
export async function fetchMyProfile(): Promise<UserProfile & { used_tokens: number }> {
  const resp = await fetch('/api/user/me', { headers: jwtHeader() });
  return parseJson(resp);
}

/** 列出当前用户的 token（含 used_tokens）。 */
export async function listMyTokens(): Promise<UserTokenMeta[]> {
  const resp = await fetch('/api/user/tokens', { headers: jwtHeader() });
  return parseJson<UserTokenMeta[]>(resp);
}

/** 创建自助 token：返回一次明文 api_key。 */
export async function createMyToken(
  req: CreateTokenRequest,
): Promise<UserTokenMeta & { api_key: string }> {
  const resp = await fetch('/api/user/tokens', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', ...jwtHeader() },
    body: JSON.stringify(req),
  });
  return parseJson(resp);
}

/** 更新自有 token（改名/额度/停启用）。 */
export async function updateMyToken(
  keyId: string,
  req: UpdateTokenRequest,
): Promise<UserTokenMeta> {
  const resp = await fetch(`/api/user/tokens/${encodeURIComponent(keyId)}`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json', ...jwtHeader() },
    body: JSON.stringify(req),
  });
  return parseJson<UserTokenMeta>(resp);
}

/** 删除自有 token。 */
export async function deleteMyToken(keyId: string): Promise<void> {
  const resp = await fetch(`/api/user/tokens/${encodeURIComponent(keyId)}`, {
    method: 'DELETE',
    headers: jwtHeader(),
  });
  if (!resp.ok) {
    await parseJson(resp);
  }
}

/** 旋转自有 token：返回新明文。 */
export async function rotateMyToken(keyId: string): Promise<{ api_key: string }> {
  const resp = await fetch(`/api/user/tokens/${encodeURIComponent(keyId)}/rotate`, {
    method: 'POST',
    headers: jwtHeader(),
  });
  return parseJson(resp);
}

// ---------------------------------------------------------------------------
// Admin user governance（仅 admin JWT；后端 403 非 admin）
// ---------------------------------------------------------------------------

/** admin 列出全部用户（不含 password_hash）。 */
export async function listAdminUsers(): Promise<AdminUserView[]> {
  const resp = await fetch('/api/user/admin/users', { headers: jwtHeader() });
  return parseJson<AdminUserView[]>(resp);
}

/** admin 创建用户（username 唯一，重复 409 username_taken）。 */
export async function createAdminUser(
  req: CreateAdminUserRequest,
): Promise<AdminUserView> {
  const resp = await fetch('/api/user/admin/users', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', ...jwtHeader() },
    body: JSON.stringify(req),
  });
  return parseJson<AdminUserView>(resp);
}

/** admin 更新用户。 */
export async function updateAdminUser(
  userId: string,
  req: UpdateAdminUserRequest,
): Promise<AdminUserView> {
  const resp = await fetch(`/api/user/admin/users/${encodeURIComponent(userId)}`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json', ...jwtHeader() },
    body: JSON.stringify(req),
  });
  return parseJson<AdminUserView>(resp);
}

/** admin 删除用户。 */
export async function deleteAdminUser(userId: string): Promise<void> {
  const resp = await fetch(`/api/user/admin/users/${encodeURIComponent(userId)}`, {
    method: 'DELETE',
    headers: jwtHeader(),
  });
  if (!resp.ok) {
    await parseJson(resp);
  }
}

/** admin 重置用户密码（token_version +1 → 旧 JWT 全失效）。 */
export async function resetAdminUserPassword(
  userId: string,
  newPassword: string,
): Promise<void> {
  const resp = await fetch(
    `/api/user/admin/users/${encodeURIComponent(userId)}/reset-password`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', ...jwtHeader() },
      body: JSON.stringify({ new_password: newPassword }),
    },
  );
  if (!resp.ok) {
    await parseJson(resp);
  }
}

/** admin 重置用户用量。 */
export async function resetAdminUserUsage(userId: string): Promise<void> {
  const resp = await fetch(
    `/api/user/admin/users/${encodeURIComponent(userId)}/reset-usage`,
    { method: 'POST', headers: jwtHeader() },
  );
  if (!resp.ok) {
    await parseJson(resp);
  }
}