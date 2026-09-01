# Agent Note: Adversarial Audit Hardening and Refactoring

Status: implemented

## Problem

经对抗性审查 Agent 团队拉网式审计，系统在核心安全、协议流式、并发性能、路由分发与终端韧性方面暴露以下缺陷：
1. **UTF-8 切片 Panic 隐患**：`sanitize_key` / `mask_key` 使用字节下标切片，遇非 ASCII / Emoji Key 引发崩溃；
2. **读路径写锁争用**：`current_state()` 在每次只读检查时强占独占写锁，高并发下导致线程饥饿与 CPU 飙升；
3. **网关路由盲选**：服务端路由使用 `providers.iter().next()` 随机挑首，多 Provider 下无视请求 `model` 产生乱路由；
4. **缺少 SSE 流式路由分支**：`stream: true` 请求被作为单次 JSON 反序列化导致 502 崩溃；
5. **配置非原子覆写**：`fs::write` 直接截断原文件，异常中断导致数据丢失；
6. **TUI 崩溃终端状态丢失**：未挂载 Panic Hook，崩溃时终端被锁死在 Raw Mode / 备用屏幕；
7. **录波无界内存占用**：未对 Snippet 进行字节长度截断，大上下文易引发 OOM。

## Decision

全面实施以下代码级加固重构：

### 1. 核心安全与并发加固 (`ponyllm-core`)
- 重构 `sanitize_key`：基于 Unicode 标量字符（`char`）安全迭代截取，统一消除各模块重复逻辑与切片 Panic；
- 限制 `FlightRecorder` 单帧采样长度为 512 字节，防止大上下文引发 OOM；
- 优化 `current_state()` 读路径：先进行只读 `RwLock::read` 快速检查，仅在冷却到期时升级为写锁；
- API Key 录入与构建请求头时执行 `trim()` 与合法性检查，杜绝静默失败。

### 2. 智能路由与 SSE 流式传输 (`ponyllm-server` & `ponyllm-protocol`)
- 在 `AppState` 中增加基于模型名称、前缀推导与默认模型映射的 `resolve_provider` 决策器；
- 在三大路由中为 `stream: true` 挂载 `execute_stream_request` 与 `axum::response::Sse` 响应管道；
- 完善多模态 Base64 与 Data URI MIME 解析。

### 3. 终端韧性与原子持久化 (`ponyllm-cli`)
- TUI 引入 RAII `TerminalGuard` 与全局 `panic_hook`，捕获 `Ctrl+C` 与异常，确保终端 100% 优雅复原；
- 配置文件持久化改为「临时文件写入 + `sync_all` + `fs::rename` 原子替换」；
- 修复 `load_or_default`，严格拦截指定路径不存在的错误；
- 移除 `wizard.rs` 中的 `Box::leak`，并优化 CLI 诊断网络异常提示。

### 4. 工程治理与测试闭环
- 修正 `.meta/gates/pre-push` 与 `pre-push.spec.sh`，消除硬编码路径并提供真实的沙箱门禁拦截负样本测试；
- 补充针对 UTF-8 Key、模型路由决策、原子写与流式分支的覆盖测试。

## Alternatives considered

- **仅修复 P0 崩溃项，搁置并发优化与原子写**：
  - *否决理由*：配置损坏与高并发锁争用在生产环境属于不可接受的隐形炸弹，必须一次性彻底加固。

## Verification

1. 任意 Unicode / Emoji Key 脱敏绝不触发 Panic；
2. 多 Provider 配置下能依据模型名正确路由到目标上游；
3. `stream: true` 请求能正常以 `text/event-stream` SSE 规范输出；
4. 配置文件保存具备崩溃原子性；
5. TUI 在 Panic 情况下终端自动恢复正常模式；
6. 全工作区测试与 L2 门禁 100% 通过。
