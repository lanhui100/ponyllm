# Agent Note: 部署冒烟门禁重试预算与 TRANSIENT/BROKEN 语义分类

Status: implemented

## Problem

在 2026-10-29 生产流水线运行（run `40839982302`）中，网关部署发布滚动与防脑裂校验全部成功通过，但在末尾执行 `scripts/post-deploy-smoke.sh` 时失败退出：
- 真实根因：上游模型提供方（sense）在突发窗口内 7+ 个 key 触发了瞬时账户限流（`EndpointTPMExceeded` / `ModelAccountRpmRateLimitExceeded`），该限流在数十秒后自动平复。
- 门禁自身实现缺陷：`post-deploy-smoke.sh` 开启了 `set -euo pipefail`，第 3 步通过管道直接将响应传给 `python3` 解析；当上游返回非 200 或非预期 JSON 结构时，管道中命令静默失败触发 `set -e` 提前退出，连错误代码与响应 body 均未能打印，直接导致流水线将外部上游瞬时限流误判为“部署失败”，引发非预期的发布告警。

## Decision

1. **解耦调用与非零保护**：
   - 使用临时文件拆分 `curl` 响应保存与状态码读取：`curl ... -o "$BODY_TMP" -w '%{http_code}' || CURL_RC=$?`。
   - 无论请求成功与否，脱敏打印 `http_code`、`curl_rc` 与响应 body 前 500 字节（不记录 Authorization 等敏感头）。
2. **两级分类退出与退避重试**：
   - **放行**：HTTP 200 且 `finish_reason` 属于 `stop` 或 `length`。
   - **瞬态重试（TRANSIENT）**：HTTP 429、5xx、`curl_rc != 0`（网络中断/超时）、非正常 JSON 响应。提供最多 3 次尝试（间隔 2s / 5s / 10s 退避）；重试耗尽后输出 `SMOKE-TRANSIENT: <status> <snippet>` 并以 `exit 1` 退出。
   - **配置损坏（BROKEN）**：其余 4xx（如 400 Bad Request / 401 Unauthorized / 403 Forbidden / 404 Not Found），代表凭据错误、端点不存在或路由契约破坏，无需重试，直接输出 `SMOKE-BROKEN: <status> <snippet>` 并以 `exit 1` 退出。
3. **离线确定性自测试金石**：
   - 新增 `tests/pre-flight/post-deploy-smoke.spec.sh`，通过 PATH 注入 mock curl 验证 ① 429 重试并成功 ② 500 耗尽打印 TRANSIENT 与 body ③ 超时 curl_rc=28 耗尽打印 TRANSIENT 与 curl_rc ④ 401 直接退出并打印 BROKEN。

## Alternatives considered

- **将 429 / 5xx 当作硬错误立即失败**：被否决。生产部署后网关面临多租户负载或上游偶发限流，冷启动瞬间若无重试窗口极易产生假阳性失败，误触发发布回滚机制。
- **无限重试直到上游恢复**：被否决。会导致 CI runner 挂死耗尽构建时长配额，违反门禁秒级/分钟级快速收敛的工程抗熵增要求。
- **吞掉 429 报错静默放行（exit 0）**：被否决。违反“零假测试与真实断言”底线，若上游已彻底断连或账号封禁，静默放行会导致严重故障外溢至生产。

## Consequences

- 部署后冒烟门禁对偶发上游抖动和限流具备自我平复能力（最高 17s 重试预算）。
- 输出日志明确区分 `SMOKE-TRANSIENT` 与 `SMOKE-BROKEN`，使下游自动回滚机制与值班排障具备清晰的机器可读语义。
