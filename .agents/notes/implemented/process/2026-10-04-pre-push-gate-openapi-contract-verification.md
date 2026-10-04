# Agent Note: 强化 pre-push 门禁对 OpenAPI 契约一致性先决校验

Status: implemented

## Problem

在多智能体协作与持续发布过程中，Rust workspace 版本号升版或 API 变更后，若开发者/智能体未主动运行 `dump_openapi_json`，会导致 `web/openapi.json` 滞后，该缺陷只有推送到远端由 GitHub Actions 跨平台测试时才会被发现并报错中断发布，增加了构建成本与发布延迟。

## Decision

在本地 Git `pre-push` 门禁（`.meta/gates/pre-push`）第一阶段中，新增 OpenAPI 契约与版本对齐断言（`cargo test -p ponyllm-server --test admin_contract_tests test_openapi_no_real_secret_and_schema_committed`）。在任何 push 触发前进行先决阻断，确保契约零破坏、本地即刻发现。

## Alternatives considered

- *方案 A：在 pre-commit 中运行*：`cargo test` 涉及编译，耗时几秒，会影响日常轻量 commit 的体验（pre-commit 定位为秒级轻量语法与 ADR 格式校验）。
- *方案 B：仅依赖远端 CI/CD*：浪费 GitHub Actions runner 配额，拉长发布闭环时间。
- *方案 C（采纳）：在 pre-push 门禁全量测试前先决校验*：既不拖慢日常 commit，又能在代码离机前百分之百拦截契约不一致。

## Consequences

- 凡是在本地执行 `git push` 时，若 `web/openapi.json` 未与当前代码/版本对齐，将被本地门禁秒级阻断并提示同步，避免远端 CI 失败。
