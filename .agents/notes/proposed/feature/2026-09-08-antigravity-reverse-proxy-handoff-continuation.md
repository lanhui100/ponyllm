# 交接文档：Antigravity 反代接入与池化调度落地 (接续指南)

Status: proposed

**更新时间**: 2026-09-08 15:05  
**代码工作区**: `/home/dm/ponyllm-antigravity` (分支: `feat/antigravity-provider`)  
**基础工程仓库**: `/home/dm/ponyllm` (main 分支保持干净无污染)  
**参考实现容器**: `gcli2api-test` (端口 `7861`, 密码 `test-pwd-7861`, 位于 Docker 容器内)  
**当前网络代理**: `http://172.17.0.1:8899` (容器内) / `http://127.0.0.1:8899` (宿主机)

---

## 一、 任务背景与阶段目标

### 1. 核心需求
根据交接需求与 `gcli2api` 实际参考实现，在独立 worktree 中实现 ponyllm 对 Google Antigravity 的反代接入：
- 支持 Antigravity OAuth 动态凭证（`access_token`, `refresh_token`, `client_id`, `client_secret`, `project_id`, `expiry`）。
- 具备并发防击穿（Singleflight 合并刷新）、提前 5 分钟缓冲、403 细分状态机（ToS 封号永久隔离、配额超限冷却、地域限制 1008 告警）。
- 双向协议转换：支持下游调用 OpenAI `/v1/chat/completions` 与 Anthropic `/v1/messages`，经 Antigravity 通道转译后返回。
- 额度与恢复时间探测：CLI `ponyllm key test` 及 Admin 拨测可获取全部 28 个模型的剩余配额百分比与北京时间恢复倒计时。

---

## 二、 当前已完成工作与成果清单

### 1. 阶段 1：对抗式审核闭环
- 由架构并发审核员与安全红队审核员完成了对抗性质询。
- 采纳防御机制：Token 刷新与配置解耦（不写回磁盘避免冲垮 `config_version` 乐观锁）、Singleflight 并发合并、严格 CLI 指纹伪装、SSE 终端帧兜底。

### 2. 阶段 2：凭证生命周期与配额探测
- **核心模块实现**:
  - `crates/ponyllm-core/src/pool/antigravity.rs`:
    - `AntigravityCredential` 支持完整字段与脱敏 `Debug`。
    - `AntigravityTokenManager` 基于 `broadcast::channel` 实现 Singleflight 刷新。
    - `fetch_quota` 探测 `/v1internal:fetchAvailableModels`，解析 `remainingFraction` 并将 `resetTime` 转换为本地北京时间。
  - `crates/ponyllm-core/src/executor/upstream.rs`:
    - 异步解 Token (`key.resolve_token().await`)。
    - 请求头注入 `User-Agent: antigravity/cli/1.1.24 windows/amd64`、`requestType: agent`、`requestId: req-{uuid}`。
    - 403 细分状态机：ToS 违规永久隔离为 `PolicyViolation`；`#3501`/`RESOURCE_EXHAUSTED` 冷却至 `resetTime`；`#1008 UNSUPPORTED_LOCATION` 临时冷却 5 分钟。
- **CLI 实测验证**:
  - 执行 `ponyllm key test --config scratch/ag-test.toml`，成功刷新并展示 28 个模型的配额与恢复倒计时（已实测验证通过）。

### 3. 阶段 3 & 4：协议转换与流式聚合 (Stream2NoStream)
- **协议转译**:
  - `crates/ponyllm-protocol/src/translator/antigravity.rs`:
    - `chat_to_antigravity_request` / `messages_to_antigravity_request`: 封装外层信封 `requestId: agent/{uuid}/{ts}/{traj}/{step}`，并使内层 `labels.trajectory_id` 严格对齐 `sessionId`（以 `-` 开头的 64 位整数字符串）。
    - `antigravity_to_chat_response` / `antigravity_to_messages_response`: 强类型双向转译。
    - `antigravity_chunk_to_chat_chunk`: 流式 chunk 强类型转译。
- **Stream2NoStream 统一流式聚合（关键突破）**:
  - **排查发现**: Google Antigravity 的传统非流式端点 `/v1internal:generateContent` 限制极严且经常抛出 429 假限流；参考实现 `gcli2api` 默认开启 `antigravity_stream2nostream = true`，无论客户端是否开启 stream，后端均请求上游 `/v1internal:streamGenerateContent?alt=sse`。
  - **解决方案**:
    - `crates/ponyllm-server/src/state.rs`: `antigravity_url` 统一返回 `streamGenerateContent?alt=sse`。
    - `crates/ponyllm-server/src/streaming.rs`: 新增 `collect_antigravity_sse_to_json` 函数，从 SSE 事件流中提取各 chunk 的 parts text、finishReason 与 usageMetadata，合并为完整 Gemini 响应。
    - `crates/ponyllm-server/src/routes/chat.rs` & `messages.rs`: 在非流式分支中，针对 Antigravity 走 `execute_stream_request` + `collect_antigravity_sse_to_json`，然后转译成标准响应返回。
- **全库测试状态**:
  - 全工程运行 `cargo test --workspace`，**103 个测试用例 100% 绿灯全部通过**（0 失败，0 警告）。

---

## 三、 关键排查发现与技术注意事项

### 1. 模型与地区风控差异
- **Gemini 模型 (如 `gemini-2.5-flash`)**: 在部分代理出口 IP 下，Google 会返回 `400 FAILED_PRECONDITION: User location is not supported for the API use` 或触发 429 配额限制。
- **Claude 模型 (如 `claude-sonnet-4-6`)**: 在同一代理节点和同一凭证下，Vertex AI Claude 通道正常放行，在参考容器 `gcli2api-test` 中请求 `claude-sonnet-4-6` 已实测返回 `1, 2, 3, 4, 5`（耗时约 2.7s）。
- **建议测试模型**: 测试时优先使用 `claude-sonnet-4-6` 进行全链路端到端验收。

### 2. 测试配置文件位置
测试配置保存在：
`/home/dm/.gemini/antigravity-cli/brain/55073343-e8c9-44b5-88ff-31e00cf84ffd/scratch/ag-test.toml`
内容如下：
```toml
[gateway]
bind = "127.0.0.1:8088"
api_key = "test-sk"
proxy = "http://172.17.0.1:8899"

[providers.antigravity]
base_url = "https://cloudcode-pa.googleapis.com"
default_model = "claude-sonnet-4-6"
default_protocol = "antigravity"
proxy = "http://172.17.0.1:8899"
models = ["claude-sonnet-4-6", "gemini-2.5-flash"]

[[providers.antigravity.keys]]
id = "ag-test-1"
priority = 100
weight = 100
api_key = '{"refresh_token":"MOCK_REFRESH_TOKEN_PLACEHOLDER","client_id":"mock_client_id_placeholder.example.com","client_secret":"REDACTED_CLIENT_SECRET_PLACEHOLDER","project_id":"aicode-consumers"}'
```

---

## Proposal

下述第四节为新窗口待执行计划（将来时）：按步骤 1–4 完成编译确认、网关拉起、四维端到端验证与交付归档；一~三节为背景与已完成事实，不属提案。

## 四、 新窗口接续后的具体执行指南 (Next Actions)

在新开窗口后，直接按照以下步骤推进：

### 步骤 1：确认工作区与编译状态
```bash
cd /home/dm/ponyllm-antigravity
git status
cargo test -p ponyllm-server test_collect_antigravity_sse_to_json
```

### 步骤 2：启动测试网关服务
确保 8088 端口未被占用，启动 ponyllm：
```bash
cargo run -p ponyllm-cli -- serve \
  --config /home/dm/.gemini/antigravity-cli/brain/55073343-e8c9-44b5-88ff-31e00cf84ffd/scratch/ag-test.toml \
  --bind 127.0.0.1:8088 --no-web
```

### 步骤 3：执行端到端四维调用验证
在另一终端发起测试验证：
1. **OpenAI 兼容端点 (非流式)**:
   ```bash
   curl -i -X POST http://127.0.0.1:8088/v1/chat/completions \
     -H "Authorization: Bearer test-sk" \
     -H "Content-Type: application/json" \
     -d '{
       "model": "claude-sonnet-4-6",
       "messages": [{"role": "user", "content": "Count from 1 to 5."}]
     }'
   ```
2. **OpenAI 兼容端点 (SSE 流式)**:
   ```bash
   curl -N -X POST http://127.0.0.1:8088/v1/chat/completions \
     -H "Authorization: Bearer test-sk" \
     -H "Content-Type: application/json" \
     -d '{
       "model": "claude-sonnet-4-6",
       "messages": [{"role": "user", "content": "Hello, stream test."}],
       "stream": true
     }'
   ```
3. **Anthropic 原生端点 (`/v1/messages`)**:
   ```bash
   curl -i -X POST http://127.0.0.1:8088/v1/messages \
     -H "x-api-key: test-sk" \
     -H "anthropic-version: 2023-06-01" \
     -H "Content-Type: application/json" \
     -d '{
       "model": "claude-sonnet-4-6",
       "max_tokens": 100,
       "messages": [{"role": "user", "content": "Hello via Anthropic protocol"}]
     }'
   ```
4. **凭证测试与配额查询**:
   ```bash
   cargo run -p ponyllm-cli -- key test \
     --config /home/dm/.gemini/antigravity-cli/brain/55073343-e8c9-44b5-88ff-31e00cf84ffd/scratch/ag-test.toml
   ```

### 步骤 4：交付归档与分支合并
1. 验证全部通过后，将设计文档从 `.agents/notes/proposed/` 迁移至 `.agents/notes/implemented/`。
2. 运行 `cargo test --workspace` 确保依然 100% 绿灯。
3. 清理测试 scratch 文件与临时服务。
4. 提交 worktree 分支 `feat/antigravity-provider` 的 commit，合入 main 分支。

## Alternatives considered

- **验收模型优先 `claude-sonnet-4-6`，而非 `gemini-2.5-flash`**：同一代理出口与同一凭证下，Vertex AI Claude 通道放行（参考容器实测返回 `1, 2, 3, 4, 5`），Gemini 模型易报 `400 FAILED_PRECONDITION` 地域限制或 429 假限流；故端到端验收优先 Claude，Gemini 仅作补充验证（见第三节）。
- **非流式走 Stream2NoStream（统一请求上游 `streamGenerateContent?alt=sse` 后聚合），而非传统 `generateContent`**：传统非流式端点限制极严且频抛 429，参考实现默认开启 `antigravity_stream2nostream`；ponyllm 在 `chat.rs` / `messages.rs` 非流式分支复用流式执行 + `collect_antigravity_sse_to_json` 聚合，否决直调非流式端点。

## Acceptance criteria

- 四维调用验证通过：OpenAI 非流式、OpenAI SSE 流式、Anthropic `/v1/messages`、CLI `key test` 28 模型配额与恢复倒计时。
- `cargo test --workspace` 100% 绿灯无回归。
- 设计文档由 `proposed/` 迁移至 `implemented/`，清理 scratch 与临时服务，`feat/antigravity-provider` 提交并合入 main。

## Risks

- **Google 内部接口与 ToS 规则突变**：字段或风控策略调整可致载荷失效；应对：对齐官方 CLI 指纹、文档明示仅供 Burner 小号实验。
- **代理出口 IP 波动**：同一模型在不同出口表现不一（地域限制 / 容量性 503）；应对：验收优先已验证模型，网关侧对瞬态错误做同 Key 退避重试。
