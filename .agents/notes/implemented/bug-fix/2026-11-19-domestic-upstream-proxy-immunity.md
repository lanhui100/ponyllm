# Agent Note: 国内与私网上游出海代理免疫与直连防御机制

Status: implemented

## Problem

在客户端（Pony Agent TUI）调用 `tokens.ponyjob.top` 网关上的国内直连模型（如 `magpie/trae-cn/deepseek-v4.1-flash-max`、`sense/deepseek-flash` 等）时，出现底层请求被错误路由至本地出海代理 `pproxy`（端口 8899）的问题。

由于大模型在生成长流式数据或深度思考（Reasoning / Thinking）时存在间歇性停顿，触发了 `pproxy` 的 `relay_idle_timeout`（30 秒空闲超时）保护机制，底层 TCP 隧道被强制掐断（日志提示 `tunnel relay deadline hit, reason="relay_idle_timeout"`），进而导致客户端 SSE 流断开并抛出 `error decoding response body`。

根本原因在于代理继承链的兜底逻辑存在防御缺失：
1. `crates/ponyllm-server/src/config.rs` 中，`effective_proxy_for_model` 的解析层级为：`model-level proxy > provider-level proxy > InheritGateway`。
2. 生产环境中网关全局配置了出海兜底代理 `proxy = "http://...:8899"`（用于部分海外服务）。
3. 当新增国内 Provider（如 `sense`：`https://token.sensenova.cn/v1`、`deepseek`：`https://api.deepseek.com`）或私网/内网 Provider（如 `magpie`：`http://100.95.193.103:3425/v1`、`workbuddy`：`http://workbuddy-host:8788/v1`）未显式配置 `proxy` 时，均错误回退至 `InheritGateway`，导致国内直连流量被强制绕行 `pproxy` 出海代理。

## Decision

确立**代码层防御性免疫 + 部署配置层显式化**的双重纵深防御：

1. **代码层直连免疫防御规则（`is_domestic_or_private_upstream`）**：
   在 `crates/ponyllm-server/src/config.rs` 中，当模型级与 Provider 级均未显式声明代理（`proxy == None`）时，检查目标上游 endpoint（`base_url`）：
   - **私网/内网/CGNAT 识别**：包含 IPv4 RFC 1918 私网网段（`10.0.0.0/8`、`172.16.0.0/12`、`192.168.0.0/16`）、Tailscale CGNAT（`100.64.0.0/10`）、回环地址、IPv6 ULA/Link-local，以及内网单段主机名（如 `workbuddy-host`、`localhost`）与内网后缀（`.local`、`.lan`、`.internal`、`.cluster.local`、`.svc`）；
   - **国内域名与主流大模型平台识别**：包含顶级域 `.cn`（如 `token.sensenova.cn`、`moonshot.cn`）及国内知名平台服务商域名（`deepseek.com`、`aliyun.com`、`aliyuncs.com`、`baidubce.com`、`baidu.com`、`volces.com`、`volcengine.com`、`minimax.chat`、`minimaxi.com`、`stepfun.com`、`01.ai`、`lingyiwanwu.com`、`xfyun.cn`、`xf-yun.com`、`tencent.com`、`tencentcloudapi.com`、`siliconflow.cn`、`siliconflow.com`、`psydo.top`）；
   - **防御性裁决**：命中上述规则的目标严禁继承网关全局出海代理，强制解析为 `EffectiveProxy::Direct`（即 `None`），直接出网。
   - **显式配置最高权威**：若运维或开发者在模型或 Provider 级显式配置了自定义代理（`proxy = "http://..."`），显式配置依然生效。

2. **配置层显式直连声明（Explicit is better than implicit）**：
   - 生产 Kubernetes Secret `ponyllm-live-config` 中，为 `magpie`、`sense`、`workbuddy`、`deepseek` 显式设置 `proxy = "direct"`；
   - 更新 `deploy/ponyllm-config.example.toml` 示例文件，注明内网及国内服务商推荐配置 `proxy = "direct"`。

## Alternatives considered

- *方案 A：仅修改 Kubernetes 生产配置中的 `proxy = "direct"`，不修改代码*。否决。仅依赖配置规范属于脆弱的人肉约定，未来若新增国内服务商（如通义、MiniMax 等）一旦遗漏 `proxy` 字段，仍会隐式回退到全局出海代理并重现该故障。
- *方案 B：直接下掉网关全局默认代理（`gateway.proxy = None`），要求所有海外模型必须逐个配代理*。否决。全局代理承载了历史兼容性与海外兜底需求，直接移除全局代理破坏了既有契约，且爆炸半径过大。
- *方案 C：调大 pproxy 的 `relay_idle_timeout`（如从 30s 提高到 10 分钟）*。否决。治标不治本。国内和内网上游走本地出海代理属于严重的路由拓扑倒退（增加延迟、增加单点故障风险），架构上严禁国内流量绕行出海代理。

## Consequences

- 国内直连模型（商汤、DeepSeek、Trae-CN 等）及内网服务（Magpie、Workbuddy 等）不再走 `pproxy`，彻底消除 30 秒空闲超时掐断问题与 SSE 断流。
- 零配置心智负担：即使未来接入新的国内或内网上游未填写 `proxy` 字段，代码级防御规则也会自动兜底为直连，杜绝意外继承出海代理。
- 单元测试与集成测试覆盖了内网 IP、Tailscale CGNAT、国内大模型域名、显式覆盖等全部场景，出网守卫断言不退化。
