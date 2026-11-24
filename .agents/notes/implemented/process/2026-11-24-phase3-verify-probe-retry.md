# Agent Note: 发布验收门禁的只读探针改有界重试（传输失败重试，判定不放宽）

Status: implemented

## Problem

`GitOps Deploy (prod)` 在 pin 冲突修好之后（PR#6），连续两次因**验收门禁的只读探测**
失败在同一类现象上，而同一时刻人工复测全部正常：

- run `45405367795` attempt 1：`== [3/7] /health` → `FAIL svc /health=000`。
  同时刻本机对 `https://tokens.ponyjob.top/health` 连测 3 次全 `200`（136–180ms）。
- 同一 run attempt 2：`== [5/7] 锁 + 刷新健康` → 首次快照为空
  （`E0=''`，`E1='0 0 0'`）→ `FAIL metrics 不可达（E0='' E1='0 0 0'）`。

两次都是 **CI runner（集群外）→ 公网入口** 的单次 `curl` 遇到传输层失败，被脚本直接判成
「部署失败」。这与 2026-10-29 冒烟门禁那次是同一类错误（当时修的是 `post-deploy-smoke.sh`
第 3 步：把上游瞬时限流判成 BROKEN），但 `scripts/phase3-verify.sh` 里的只读探测
（`/health`、`/v1/telemetry/metrics` 的四处采集）仍是**单次 curl**，没有重试预算。

判定口径本身是对的（`[5/7]` 的「双空快照不得平凡通过」是有意为之，防的是
`E0=''` + `E1=''` → `Δ==0` 假绿）；缺的是「一次传输抖动」与「端点真的不可达」的区分手段。

## Decision

抽 `scripts/lib/http-retry.sh`，两个函数，只对**传输层失败**重试：

| 函数 | 重试条件 | 不重试条件 |
|---|---|---|
| `http_code_retry <url> [max_time] [attempts] [token]` | curl 退出码 ≠ 0 或 HTTP 码为 `000` | 拿到任何非 `000` 的码（含 4xx/5xx——那是真实响应） |
| `http_body_retry <url> [max_time] [attempts] [token]` | 响应体为空 | 响应体非空（JSON 合法性由调用方判定） |

- 默认 `attempts=3`，间隔 2s（总预算 ≤ 4s + 3×max_time，远小于 job 超时）。
- 重试耗尽仍 `000` / 仍空 → 原样返回 `000` / 空串，**调用方照旧 FAIL**（fail-closed，不静默放行）。
- `phase3-verify.sh` 的四处只读采集（`[3/7]` svc `/health`、`[4/7]` reload 前后、
  `[5/7]` 锁快照）改用该库；`[5/7]` 的「双空即 FAIL」判据**原样保留**。
- 库以 `. "$_P3_DIR/lib/http-retry.sh"` 方式被 source（不新增可执行入口），
  自测 `tests/pre-flight/http-retry.spec.sh`（P1–P7）接入 CI `test` job。

## Alternatives considered

- **把 `[3/7]` / `[5/7]` 的 FAIL 降级成 WARN**（落选）：等于用「关掉看守」换绿灯。
  `/health` 与锁错误 delta 是发布验收的核心断言，降级后真故障也会被放行。
- **把重试逻辑直接内联进 `phase3-verify.sh` 四处**（落选）：四处重复同一段循环，
  改口径要改四处；抽库后「重试什么、不重试什么」只有一份实现与一份自测。
- **对所有失败（含 4xx/5xx）一律重试**（落选）：真实 4xx 是契约破损（端点不存在、
  凭据错误），重试只是把 3 秒的失败拖成 30 秒，且会掩盖契约漂移——`post-deploy-smoke.sh`
  的 `SMOKE-BROKEN` 语义已经明确不重试这类。
- **拉长 `--max-time` 而不重试**（落选）：本次是链路瞬断不是慢响应；加超时对断链无效，
  反而让真故障的失败反馈变慢。
- **顺手也给 `post-deploy-smoke.sh` 的 `/health` 加重试**（本批不做）：该步在两次失败运行里
  都是 `OK: /health 200`，且其 `.spec.sh` 用 mock curl 断言调用次数，改动会牵动既有负样本；
  留待下次触碰该脚本时一并对齐，避免本 PR 跨两个门禁面。

## Consequences

- 公网链路偶发断链不再把「部署成功」误判成「部署失败」：两次失败运行的现象在重试预算内可自愈。
- 真不可达仍然红（`000` / 空体照旧 FAIL），且日志里能看到它试了 3 次——比单次更可诊断。
- 判定口径零放宽：`[5/7]` 的「双空快照不得平凡通过」、`[3/7]` 的 `!=200 即 FAIL` 均原样保留。
- 库的自测（P1–P7）把「重试次数」也纳入断言：只验最终结果会漏掉「其实没重试」的假实现。
