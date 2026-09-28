# Agent Note: 同名模型跨 provider 的显式优先级（model priority）

Status: implemented

## Problem

同名模型（如 `gpt-6-sol`）可以合法注册在多个 provider 下，ponyllm 把它们收集为
多候选透明 failover 链。候选顺序完全由策略评分决定（Economy=价格 / Speed=延迟 /
Balanced=价格+延迟 / Reliable=活跃 key 数），并叠加 hot cache 与同协议直通平局键。
这带来两个痛点：

1. 运营商无法显式表达"同一模型优先用哪家 provider"。价格全等时顺序还取决于
   `HashMap` 迭代顺序（不确定），Ops 无法稳定复现"首选谁"。
2. 想表达偏好的近似手段（调低目标 provider 的 `input_price`、用
   `x-pony-strategy: reliable`）语义污染：价格是计费数据，不该兼职路由权重；
   reliable 依赖活跃 key 数，不可精确控制。

需要一个 per-(provider, model) 的显式优先级字段：配置在哪个 provider 的哪个模型上，
只影响该 provider 在该模型下的候选排序，不影响计费语义。

## Decision

新增模型级 `priority: Option<u32>`（数值越大越优先；`None` 视作 0，即无偏好），
贯穿配置 → 运行时 → 路由：

- `ponyllm_config::ModelConfig`（磁盘 TOML 格式）与
  `ponyllm_server::config::ModelSpec`（运行时格式）各自增加
  `#[serde(default, skip_serializing_if = "Option::is_none")] priority: Option<u32>`，
  二者在 CLI 的 `build_gateway_config_and_pools` 与 admin 写路径上原样透传。
- `RoutedTarget` 携带 `priority`；`sort_candidates` 在既有 passthrough 平局键与
  策略评分**之前**做一次稳定降序预排序：优先级不同的候选，优先级高者恒排在前面；
  优先级相同（或都未配置）的候选，完全走既有逻辑（passthrough 直通 → 策略评分 →
  hot cache），行为与未引入该字段时逐位一致。
- admin API：`CreateModelPayload` / `UpdateModelPayload` / `ModelView` 增加
  `priority`；Web 控制台模型新建/编辑表单可填可改，模型列表展示优先级徽标。
- 语义排序：**显式优先级 > hot cache > 策略评分**。显式优先级是运营商意图的
  硬性表达；hot cache（省钱粘性）只在同优先级候选内生效。
- failover 语义保留：高优先级 provider 的 key 池熔断/耗尽后，请求自动降级到低
  优先级候选，与既有逐候选重试链一致。

## Alternatives considered

1. **DSH 侧 route headers 下发 `x-pony-provider` 钉住 provider**——模型 id 保持
   干净，但需要 ponyllm 新增请求头解析与候选过滤（另一处行为变更），且表达的是
   "这次指定谁"而非"平时优先谁"，与 failover 自动降级正交。列为后续可选增强
   （Phase 1），本次不做。
2. **`provider/model` 前缀语法钉死**——零代码，但模型 id 变成复合串，响应体
   回显、日志、计量全部带前缀，且 `/v1/models` 不列出复合 id、DSH discovery
   发现不到；作为"临时指定"手段保留，不作为优先级机制。
3. **调低目标 provider 同模型的 `input_price` 让 Economy 首选**——零代码近似，
   但把计费数据当路由权重，成本统计口径被污染；且只有 Economy 策略下成立。
4. **`x-pony-strategy: reliable` 近似**——依赖 key 池活跃数，无法精确、稳定地
   表达偏好。
5. **`priority` 仅作平局键而非第一排序键**——与需求语义不符：用户要的是
   "首选 A、备选 B"，而非"价格相同时才看优先级"。
6. **provider 级优先级而非模型级**——粒度不足：同一 provider 下不同模型需要
   不同优先级（某模型 A 首选、另一模型 B 落选），provider 级无法表达。

## Consequences

- 未配置 `priority`（既有配置零迁移）：字段缺省 `None` → 视作 0，排序行为与
  变更前逐位一致。
- 配置了优先级的部署：同名模型跨 provider 时首选可稳定复现，熔断后自动降级。
- 显式优先级会覆盖 hot cache 的经济粘性（同优先级组内不受影响）——文档需写明
  这是"运营商意图优先于自动择优"的取舍。
- admin API/契约与 Web 表单同步扩展，`openapi.json` 需随 schema 变更重新生成。