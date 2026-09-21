---
name: ponyllm-quota
description: |
  ponyllm 网关额度查询 skill：查"模型现在能不能用"（按 provider/key 的可用性快照）。

  触发场景：
  - 查额度："deepseek 还剩多少额度？"、"antigravity 的 key 还能用吗？"
  - 选模型前探活："现在哪个 provider 可用？"、"这个 key 是不是在冷却？"
  - 故障排查："为什么请求被倒换了？"、"key 的冷却是多久恢复？"
---

# ponyllm-quota — 网关额度查询

只读入口，实时数据永远走网关 `GET /api/admin/quota`（本 skill 不存额度、不直调上游）。

## 前置

- 网关已启动（默认 `http://127.0.0.1:8080`），手里有网关分级 key（`ponyllm keys issue` 签发）。
- **分级 key 指引（P1/P2）：agent 只领 `inference`（`sk-pony-infer-…`）**——可调推理 + 读 quota/telemetry 摘要，永不能调 `auth/rotate`/用户管理；`readonly`（`sk-pony-read-…`）不能推理；`admin`（`sk-pony-admin-…`）全权，人用不落地 agent。旧版单 token 在 `dual` 下仍可用（打标 `deprecated-auth`），切 `strict` 后一律 401，需重领分级 key。
- 接口需鉴权：`Authorization: Bearer <网关分级key>` 或 `X-Api-Key: <网关分级key>`（`strict` 下裸 token 无 `Bearer ` 前缀会被拒绝；`?token=` query 鉴权已禁用，不要拼进 URL）。
- 401 vs 403：401 = 凭证错/过期/legacy 已关（换 key 或重领）；403 `insufficient_scope` = key 作用域不够（提权换 key，不要重试）。

## 常用查询（把 `<GW>` 换成网关地址，`<KEY>` 换成网关 inference key）

```bash
# 1. 全快照：所有 provider/key 的状态（默认纯内存，零上游调用）
curl -s -H "Authorization: Bearer <KEY>" '<GW>/api/admin/quota' | head -c 2000

# 2. 只看某 provider
curl -s -H "Authorization: Bearer <KEY>" '<GW>/api/admin/quota?provider=deepseek'

# 3. 只看某 key（含冷却恢复时间）
curl -s -H "Authorization: Bearer <KEY>" '<GW>/api/admin/quota?key_id=<key-id>'

# 4. Antigravity 穿透刷新（走一次上游 quota 探针，失败自动降级 stale）
curl -s -H "Authorization: Bearer <KEY>" '<GW>/api/admin/quota?provider=antigravity&refresh=true'
```

## 字段速查

- `state`: `active`（可接流量）/ `cooling_down`（冷却中，看 `cooldown_reset_at`）/ `disabled`（已下线）
- `schedulable`: 是否可调度（`true/false`）
- `source`: 信号血缘——`probe_only`（内存态）/ `buckets`（antigravity 上游快照）/ `unknown`
- `stale: true`：刷新探针失败，返回的是内存旧态
- 响应永不含 key 原文（只有 key id + 状态）

## 判读规则

1. 先看 `state` + `schedulable`，再看 `cooldown_reset_at`（恢复时间，UTC）。
2. `source=probe_only` 是"存活/冷却态"，不是"剩余额度"——不要把"active"读成"有钱"。
3. `antigravity` 要看余量必须加 `refresh=true`（`buckets` + `quota_groups` 的 5h/weekly 桶）。
4. 深度诊断（按 key 拨测）走 `POST /api/admin/keys/{id}/test`（需 `admin_write_enabled`）。
