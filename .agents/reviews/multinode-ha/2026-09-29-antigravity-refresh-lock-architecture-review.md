# 对抗审核报告：Antigravity HA 刷新锁竞争修复与一致性审查

- **任务编号**：`task-1` (架构与一致性对抗审核)
- **审查员**：`arch-reviewer`
- **目标代码**：
  - `crates/ponyllm-core/src/pool/antigravity.rs`
  - `crates/ponyllm-server/src/routes/admin.rs`
  - 关联组件：`crates/ponyllm-cli/src/main.rs` (ConfigPoller/reload)、`crates/ponyllm-core/src/executor/upstream.rs`
- **审核结论**：**发现严重架构缺陷（S1 × 1，S2 × 2），当前修复存在核心机制失效，需阻断并重构修复方案**。

---

## 核心发现与问题分级

### 【S1 致命缺陷】`get_valid_token_with_retry` 轮询检查 `self.cred` 属于虚空等待：跨副本刷新根本无法更新当前 `self.cred` 实例！

#### 1. 证据链分析
1. **Secret 写入内容仅含 `refresh_token`**：
   在 `crates/ponyllm-server/src/state.rs:893`，持锁副本刷新成功后，回写到 K8s Secret `ponyllm.toml` 的仅仅是：
   ```rust
   k.api_key = token.clone(); // 注意：token 是 refresh_token，不是 access_token！
   ```
   **K8s Secret 中根本不保存 `access_token`，只保存长期凭据 `refresh_token`**！
2. **ConfigPoller 热重载创建全新实例，不修改老实例的 `self.cred`**：
   当另一个副本写回 Secret 触发当前副本的 ConfigPoller（2000ms 周期）时，`main.rs:428-436` 执行：
   ```rust
   let (new_gw_cfg, new_pools) = build_gateway_config_and_pools(...);
   st.reload_config_with_pools(new_gw_cfg, new_pools);
   ```
   它通过 `build_gateway_config_and_pools` 创建了**全新的 `AntigravityTokenManager` 和全新的 `KeyPool`**，并且新实例初始时 `access_token: None`。
3. **老实例孤立**：
   当前正在执行 `get_valid_token_with_retry` 的线程所持有的 `self` 是**老 Pool 里的老 `mgr` 实例**。
   在 `get_valid_token_with_retry` 的 retry 循环中：
   ```rust
   let snapshot = self.cred.read().clone();
   if !Self::credential_needs_refresh(&snapshot) {
       if let Some(token) = snapshot.access_token { ... }
   }
   ```
   - 另一个副本更新的是 K8s Secret 里的 `refresh_token`；
   - 没有任何机制会把另一个副本生成的 `access_token` 发送给当前副本；
   - 当前副本的 `self.cred` 中的 `access_token` **永远是 None 或已过期**；
   - 只要当前副本拿不到全局锁，`self.cred` 里的 `access_token` 就**绝对不会**神奇地变成有效！

#### 2. 实际执行路径与后果
在 `get_valid_token_with_retry` 中，第一段 `self.cred.read()` 检查永远为假，每次循环都会落入：
```rust
match self.get_valid_token_inner(false).await {
    Ok(token) => return Ok(token),
    Err(CoreError::RefreshSkipped { .. }) => continue,
    Err(e) => return Err(e),
}
```
这实际上退化成：**在当前副本自身反复去竞争全局 PG 排他锁**！
- 如果另一个副本持锁没释放，当前副本在这 6 秒内重试 6 次，全部被 `try_acquire` 拒绝；
- 如果另一个副本释放了锁，当前副本终于抢到了锁，然后**由当前副本亲自去调用一次 Google OAuth 端点刷新 token**！
- 这直接违背了代码注释宣称的 “waiting for lock holder to finish refreshing and propagating the token” —— 根本没有 token 跨副本传播机制！

---

### 【S2 严重缺陷】重试退避上限（6s）导致上游请求挂起与级联超时风险

#### 1. 证据链分析
1. **客户端与网关超时预算紧绷**：
   `DEFAULT_UPSTREAM_TTFB_TIMEOUT` 为 15s（`crates/ponyllm-core/src/executor/upstream.rs:605`）。
2. **多 Key 级联放大**：
   在 `UpstreamExecutor::execute_with_retry` 中，当池中有多个 Antigravity Key 处于冷启动或需要换 Token 时，每个 Key 在 `build_headers` 时调用 `resolve_token()`。
   如果每个 Key 都退避等待 6s，前 2 个 Key 遇到锁冲突就会累计耗时 12s，第 3 个 Key 耗时达到 18s，直接击穿上游 TTFB 超时或客户端连接超时，导致下游调用方（如 DSH）直接断连报错。
3. **退避序列过长且缺乏抖动（Jitter）**：
   `[200, 400, 800, 1200, 1500, 2000]` 的固定阶梯退避在多副本并发时会导致惊群效应（所有等待副本在相同时间点同时发起 `try_acquire` 争抢锁）。

---

### 【S2 严重缺陷】全局单锁模型在“多账号”场景下的架构不匹配（风控伪假设）

#### 1. 证据链分析
`REFRESH_LOCK_KEY` 采用全局单锁：
```rust
const REFRESH_LOCK_KEY: &str = "ponyllm-antigravity-refresh";
```
ADR 中假设“同出口 IP 下多账号并发刷新会触发风控”。然而在现实中：
- 算力池中配置了 8~15 个不同的 Google 账户（如 `ag-telfersean464@gmail.com`、`ag-city8585378@gmail.com` 等）；
- 当 A 账户需要刷新时，全局锁被占用；此时 B 账户的请求到来，**即使 B 账户的 refresh_token 完全正常，B 账户也被迫等待甚至报错 `RefreshSkipped`**！
- 这导致**任意一个账号刷新慢或网络阻塞，整个网关所有账号全部被锁死**！这是此前 DSH 报错 `Antigravity refresh for 'ag-city8585378@gmail.com' skipped: serialization lock held by another replica` 且级联遍历 8 个 Key 全部失败的根本原因！

---

## 架构整改建议与修复方案 (Actionable Recommendations)

1. **修正全局锁粒度：由全局单锁改为“账号级锁 (per-key advisory lock)”**：
   - 跨副本排他锁的真正目的是防止**同一个账户**被多个副本并发刷新造成 refresh_token 旋转竞争（`invalid_grant`）；
   - 不同 Google 账号之间互不相干，不应互相阻塞。将 PG advisory lock 的 key 改为基于 `key_id` 的哈希：
     ```rust
     format!("ponyllm-antigravity-refresh:{}", key_id)
     ```
   - 这样 A 账号刷新绝不影响 B 账号！从根本上消除多账号级联跳过问题。
2. **重构 `get_valid_token_with_retry` 语义**：
   - 认识到当前架构下 `access_token` 不会跨副本同步（仅通过 Secret 同步 `refresh_token`）；
   - 等待锁的目的只能是：等待当前正在刷新该账号的副本完成并释放锁，随后本副本以最新轮转的 `refresh_token` 获取锁并刷新，或者等待本副本内的 Singleflight 广播；
   - 缩减等待时间：最大退避时间缩减至 1.5s ~ 2.5s（如 3 次尝试：100ms, 300ms, 600ms + random jitter），防止拖垮请求管线。
3. **保持后端 429 `lock_busy` 与前端色块解耦**：
   - 当前在 `admin.rs` 将 `RefreshSkipped` 归入 `lock_busy`、前端不误报 `invalid_grant` 的方向是完全正确的，必须保留。
