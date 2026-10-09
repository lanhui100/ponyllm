# Agent Note: user management and quota limits

Status: implemented

## Problem

ponyllm 当前的网关凭据体系仅支持扁平的 `GatewayKeyEntry`（含 `admin`, `inference`, `readonly` 三种 scope），缺少面向业务单个用户的“用户（User）”实体抽象。这导致以下局限：
1. 无法按租户/个人用户进行聚合管理与鉴权（例如：属于同一用户的多个 API Key 共享额度，或用户直接作为认证主体）。
2. 无法为特定用户设置 Token 用量上限（例如每月/总 token 预算，用尽即拒绝请求）。
3. 无法限制特定用户能够访问的模型集合（模型白名单或通配匹配，例如用户 A 只能用 `gpt-4o-mini`，不能用 `claude-3-5-sonnet` 或 `gemini-1.5-pro`）。
4. 缺少用户层级的管理与计量接口（Admin CUD API），无法支持多用户租户化运营。

## Decision

在 `ponyllm-config`、`ponyllm-core` 和 `ponyllm-server` 中引入第一公民级用户管理子系统：

### 1. 配置模型与数据结构 (`ponyllm-config` & `ponyllm-core`)
新增 `UserEntry` 实体：
```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserEntry {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_user_enabled")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_models: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(default = "default_user_created_at")]
    pub created_at: i64,
}
```

并在 `GatewayKeyEntry` 中增加可选字段：
```rust
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
```
并在 `ConfigFile` / `GatewayConfig` 中增加 `pub users: Vec<UserEntry>`。

### 2. 运行时用户状态与 Token 用量追踪 (`ponyllm-core`)
实现无锁并发安全的 `UserQuotaTracker`：
- 每个用户维护原子累计已消耗 tokens（`AtomicU64`）。
- 准入校验：检查 `enabled`、模型白名单匹配（支持精准匹配及 `*` 通配符）与当前已用 tokens 是否超限。
- 记账机制：推理完成后通过 `record_tokens` 累加实际使用量。

### 3. 请求上下文透传与鉴权中间件扩展 (`ponyllm-server`)
- `auth_middleware` 在校验 `GatewayKeyEntry` 时提取 `user_id`，注入到请求头 `x-user-id` 与 `CallerIdentity` 扩展。
- 推理入口（`/v1/chat/completions`、`/v1/messages`、`/v1/responses`）前置检查用户权限与额度，完成后自动累加用户消耗 Token。

### 4. Admin 管理 API 接口 (`ponyllm-server`)
在 `/api/admin/` 下暴露完整 Users 管理与用量重置端点：
- `GET /api/admin/users`: 列出所有用户及当前 Token 消耗用量
- `POST /api/admin/users`: 创建用户
- `GET /api/admin/users/{id}`: 获取用户详情与配额状态
- `PUT /api/admin/users/{id}`: 更新用户配置
- `DELETE /api/admin/users/{id}`: 删除用户
- `POST /api/admin/users/{id}/reset-usage`: 重置用户的 Token 消耗量计数

## Alternatives considered

1. **复用 `GatewayKeyEntry` 字段而不引入 User 实体**：
   - *劣势*：一个用户可能持有多个不同客户端/设备的 Key，如果把 max_tokens 和 allowed_models 散落在每个 key 上，无法实现用户维度的统一用量限额、统一停用与统一模型管控。
2. **完全依赖外部商业 PostgreSQL 账本 (ponyllm-billing)**：
   - *劣势*：目前 ponyllm-billing 尚处于 Stage 0 / 早期，多数本地化和轻量网关以单机 toml/json/k8s Secret 为真值源。用户系统应作为网关原生轻量核心能力（零外部 DB 依赖也可工作），未来再挂接商业级 PG。

## Consequences

- 实现了对单个用户的精细化模型权限与 Token 用量管控。
- 保持与现有 `GatewayKeyEntry` 和 `ConfigFile` 的平滑向下兼容。
- 零外部数据库依赖，运行时无锁并发安全（DashMap + AtomicU64）。
