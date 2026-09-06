# Agent Note: Upstream Proxy Isolation and Per-Provider Configuration

Status: implemented

## Problem

PonyLLM 作为大模型路由网关，其上游 HTTP 客户端以往直接使用 `reqwest::Client::builder().build()` 构建。在默认情况下，reqwest 会自动从宿主进程环境变量中继承 `http_proxy`、`https_proxy` 与 `all_proxy`。

当宿主机开启了本地开发代理工具（如 `pproxy on`，将环境变量代理指向本地白名单出海端口 `127.0.0.1:8899`）时，网关发往所有外部上游的流量都会被强制走本地代理。由于国内大模型（如商汤 SenseNova、月之暗面 Moonshot、阿里百炼、深度求索 DeepSeek、自建代理中转等）未配置在该白名单路由中，代理服务直接返回 403 `no_tunnel_route` 掐断握手，导致网关报 503 `Network error`。

此前的临时应对是在宿主机环境变量中把每个新增 Provider 的域名手工追加至 `NO_PROXY`。但这种做法不仅将网关服务与宿主机交互式终端的运维状态强耦合，而且任何新 Provider 接入都需要侵入宿主机网络环境变量，存在严重维护债务且极易再次踩坑。

## Decision

彻底在代码架构层实现网关网络环境解耦与 Provider 级精准代理治理：

1. **默认上游网络环境解耦（Default Isolation via `.no_proxy()`）**：
   在 `crates/ponyllm-core/src/executor/upstream.rs` 中，`create_upstream_http_client` 与 `create_upstream_http_client_with_options` 默认开启 `builder.no_proxy()`，完全屏蔽操作系统环境中的 `http_proxy`/`https_proxy`，确保所有直连 Provider（国内/私有/自建 VPC）天然直通网络，绝不受终端代理污染，再也无需维护宿主 `NO_PROXY`。

2. **支持显式 Provider 级代理配置（Per-Provider Proxy）**：
   在 `ProviderConfig`（及 CLI `ProviderSection`）中引入可选字段 `proxy: Option<String>`。若特定 Provider 需要走代理（例如仅 OpenAI 需经过出海隧道 `http://127.0.0.1:8899`），则在配置中显式声明：
   ```toml
   [providers.openai]
   base_url = "https://api.openai.com/v1"
   proxy = "http://127.0.0.1:8899"
   ```

3. **服务端动态分发与热重载支持（`http_client_for_provider`）**：
   在 `crates/ponyllm-server/src/state.rs` 的 `AppState` 中维护针对配置了独立代理的 Provider 的专属 `reqwest::Client` 集合，对外暴露 `http_client_for_provider(&provider_name)`。在配置热重载（`reload_config_with_pools`）时动态同步更新；路由模块（`chat.rs`、`responses.rs`、`messages.rs`）统一按请求目标 Provider 检索专用客户端。

4. **网关全局代理与宿主继承逃生舱（Gateway Fallback & Escape Hatch）**：
   在 `GatewayConfig`（及 CLI `GatewaySection`）中引入：
   - `proxy: Option<String>`：为未显式配置代理的 Provider 提供统一默认代理（可选）。
   - `use_system_proxy: bool`（默认 `false`）：为极少数确实依赖容器环境注入全局代理的场景保留显式开启系统环境变量代理的逃生通道。

## Alternatives considered

- *方案 A：继续依赖宿主机维护 `NO_PROXY`*。否决。将反向代理网关的上游网络链路绑定在宿主机交互式 shell 的环境变量上是架构反模式，违背自包含与高可用原则，每次新增国内/自建 provider 都会再次诱发生产级网络故障。
- *方案 B：仅在网关启动脚本中使用 `env -u http_proxy` 剥离环境变量*。否决。这仅仅将治理负担转嫁给了部署运维，无法在代码和配置文件层面提供对真正需要翻墙/走专线代理的 Provider（如海外官方 API）提供粒度化控制。
- *方案 C：单个全局代理，所有上游一刀切*。否决。LLM 网关常见拓扑是同时混部国内模型（商汤/通义/DeepSeek，直连最低延迟）和海外模型（OpenAI/Anthropic，需代理出海）。全局一刀切代理必然导致直连流量绕行甚至像 pproxy 那样被白名单拒载。

## Consequences

- 网关上游 HTTP 客户端默认纯净直连，宿主机无论是否执行 `pproxy on`，直连模型（如商汤 SenseNova、DeepSeek 等）均稳定直达，再也不需要向系统环境变量写入 `NO_PROXY`。
- 允许在 `ponyllm.toml` 中按 Provider 或网关层显式声明 `proxy`，网络拓扑完全透明、代码与配置版本化可控。
