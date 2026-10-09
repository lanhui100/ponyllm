// B003 红相最小桩（仅契约面，无业务实现）：/api/user 前端封装。
//
// 冻结契约（Lead B003 派单 d 项，对齐 B002 后端）：
// - loginWithPassword: POST /api/user/login {username,password} → {access_token, user}
// - 自助 token CRUD/rotate：/api/user/tokens...
// 当前不存在实现 → 每个函数 throw（红相：调用即失败）；绿相由实现者替换为真实
// alova/fetch 封装（签名与类型为本文件冻结面，绿相不得破坏）。
// 严禁此文件触碰 wave-2 在途文件（adminApi.ts / types/admin.ts 等）。

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

/** 口令登录：POST /api/user/login → access_token（JWT 仅内存，调用方自行入 session store）。 */
export function loginWithPassword(username: string, password: string): Promise<LoginResponse> {
  void username;
  void password;
  throw new Error('B003 not implemented: /api/user/login wiring');
}

/** 列出当前用户的 token（含 used_tokens）。 */
export function listMyTokens(): Promise<UserTokenMeta[]> {
  throw new Error('B003 not implemented: GET /api/user/tokens');
}

/** 创建自助 token：返回一次明文 api_key。 */
export function createMyToken(req: CreateTokenRequest): Promise<UserTokenMeta & { api_key: string }> {
  void req;
  throw new Error('B003 not implemented: POST /api/user/tokens');
}

/** 更新自有 token（改名/额度/停启用）。 */
export function updateMyToken(keyId: string, req: UpdateTokenRequest): Promise<UserTokenMeta> {
  void keyId;
  void req;
  throw new Error('B003 not implemented: PUT /api/user/tokens/{key_id}');
}

/** 删除自有 token。 */
export function deleteMyToken(keyId: string): Promise<void> {
  void keyId;
  throw new Error('B003 not implemented: DELETE /api/user/tokens/{key_id}');
}

/** 旋转自有 token：返回新明文。 */
export function rotateMyToken(keyId: string): Promise<{ api_key: string }> {
  void keyId;
  throw new Error('B003 not implemented: POST /api/user/tokens/{key_id}/rotate');
}
