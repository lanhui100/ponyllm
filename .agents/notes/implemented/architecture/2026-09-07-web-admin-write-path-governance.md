# Agent Note: Web控制台Admin写路径与治理

Status: implemented

## Problem

Web 控制台在 M3 仅交付了读端点与全局路由策略写端点，Provider、Model、Key 的增删改（CUD）与拨测能力尚未打通，运维人员仍需手工修改配置文件。
此外，写路径缺乏治理保障：
1. **并发竞态覆盖**：多管理员并发编辑或外部文件被同时修改时，缺少乐观锁机制会导致改动被无预警覆盖；
2. **缺少配置备份与容灾**：保存新配置前若无自动备份，非法改动或意外中断会导致原配置损坏且不可逆；
3. **缺少生产环境写安全开关**：写路径端点若未经显式白名单控制，容易在生产环境导致误操作，缺少灰度开关；
4. **敏感密钥扩散**：新 Key 录入若长期在管理端点回显明文，存在严重的凭证外泄风险。

## Decision

本期落地 WEB-06 Admin 写路径全链与关键治理机制：
1. **四项核心治理债交付**：
   - **写前自动备份**：在落盘保存新配置前，自动将当前配置文件备份为 `.bak` 文件（如 `ponyllm.toml.bak`），确保提供即时还原能力。
   - **`If-Match` 乐观并发控制**：所有写请求（PUT / POST / DELETE）强制校验 `If-Match: "<config_version>"` 请求头；若版本号不匹配，立即返回 `412 Precondition Failed`，杜绝覆盖并发修改。
   - **串行化写队列与审计日志**：基于互斥锁串行化配置的“加载-修改-验证-落盘-热重载”全链路，彻底消除竞态窗口；记录操作人与变更实体的审计日志。
   - **灰度开关 `admin_write_enabled`**：在 `GatewayConfig` 增加开关，默认 `false`；未开启时所有 CUD 接口返回 `404 Not Found` 并记录审计拦截。
2. **Admin CUD 与实时拨测端点**：
   - `POST /api/admin/providers`：新增 Provider 配置。
   - `DELETE /api/admin/providers/{name}`：删除 Provider 及关联资源。
   - `POST /api/admin/models`：新增 Model。
   - `PUT /api/admin/models/{name}`：更新 Model（如 thinking 参数）。
   - `DELETE /api/admin/models/{name}`：删除 Model。
   - `POST /api/admin/keys`：新增 Key，**明文 API Key 仅在创建响应中一次性回显**，后续读接口维持脱敏。
   - `DELETE /api/admin/keys/{id}`：删除 Key 并热同步连接池。
   - `POST /api/admin/keys/{id}/test`：Key 拨测接口，向目标 Provider 发起带严格超时（3s）的探针，返回结构化状态与时延，日志严格脱敏。
3. **OpenAPI 契约更新**：
   - 使用 `utoipa` 注解声明上述所有新增端点、Payload 与 Response Schema，导出并提交最新 `web/openapi.json`。

## Alternatives considered

- **不加写队列直接靠文件系统锁：否定。文件锁存在跨平台差异且无法保护内存缓存状态一致性，应用层异步互斥锁是最小成本且确定的最优解。**
- **新建 Key 在后续列表中仍可查看明文：否定。违背安全最小权限原则，明文只在创建瞬间一次性回显，后续接口一律脱敏。**
- **拨测调用真实大模型长上下文：否定。成本高且耗时长，拨测使用最小探活请求并设置 3s 硬超时。**

## Consequences

- `admin_write_enabled=false` 时，所有 CUD 写端点及拨测返回 404；显式开启后方可执行。
- `If-Match` 乐观并发控制生效，版本不符时返回 412，串行化互斥锁彻底消除了多请求写入与配置持久化的并发竞态。
- 每次写配置落盘前自动生成有效 `.bak` 文件，具备即时容灾备份能力。
- 新增 Key 明文仅在创建瞬间一次性回显，后续 GET `/api/admin/keys` 严格脱敏掩码，杜绝凭证外泄。
- `keys/{id}/test` 拨测覆盖正常与异常（401/429/超时）Schema，并受 3s 独立 Client 硬超时保护。
- `web/openapi.json` utoipa 契约 100% 覆盖新增端点与 Schema，成为前端管理视图消费的标准契约。
