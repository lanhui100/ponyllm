# Agent Note: Layered Gates and CI Release Pipeline

Status: implemented

## Problem

软件工程的门禁如果缺乏分层，容易陷入两难困境：
1. **全部堆在 pre-commit**：导致每次本地 `git commit` 耗时数十秒甚至几分钟，开发者心流被严重打断，最终诱使开发者滥用 `--no-verify` 绕过门禁；
2. **全部交给云端 CI**：本地提交毫无拦截，破损的代码推送到远程后才报红，修复延迟高且污染分支历史；
3. **缺乏自动化发版流水线**：打 Tag 后人工打包分发低效且易出错。

## Decision

我们构建了 **pre-commit（秒级）➔ pre-push（10秒级）➔ CI/Release（分钟级）** 三级分层物理门禁矩阵：

### 1. 门禁分级矩阵

| 门禁层级 | 执行时机 | 耗时预算 | 拦截内容 | 退出保障 |
| :--- | :--- | :--- | :--- | :--- |
| **`pre-commit`** | 本地 `git commit` | **秒级 (~1-2s)** | ADR 正向影子校验、ADR 负样本拒绝回归、`cargo check --workspace` 语法编译检查。 | 非零拦截 |
| **`pre-push`** | 本地 `git push` | **10秒级 (~5-15s)** | 工作区全量 27 项单元/集成测试（`cargo test --workspace`）、`ponygo status` L2 自洽判定、门禁负样本 spec 校验。 | 非零拦截 |
| **`CI`** | GitHub Actions (Push/PR) | **分钟级 (~1-3m)** | Linux/macOS/Windows 三平台矩阵编译、全量测试与 ADR 治理校验。 | Status Check 阻塞合并 |
| **`Release`** | GitHub Actions (Tag `v*`) | **分钟级** | 自动构建跨平台二进制发布包并创建 GitHub Release。 | 自动发布 |

### 2. 门禁与负样本规格文件布局

- `.meta/gates/pre-commit` 与 `.meta/gates/pre-commit.spec.sh`
- `.meta/gates/pre-push` 与 `.meta/gates/pre-push.spec.sh`
- `.github/workflows/ci.yml`
- `.github/workflows/release.yml`

## Alternatives considered

- **单层 pre-commit 运行全部测试与静态检查**：
  - *否决理由*：随着测试用例增加，commit 延迟会指数上升，开发者难以忍受卡顿。
- **纯靠云端 CI 拦截**：
  - *否决理由*：无法在代码离开开发者机器前形成第一道物理防线。

## Consequences

- 实现了极速反馈（pre-commit 秒级）与严格守卫（pre-push 10秒级、CI 分钟级）的绝佳平衡；
- 远程开放仓库与 GitHub Actions 自动化发布流程全线贯通。
