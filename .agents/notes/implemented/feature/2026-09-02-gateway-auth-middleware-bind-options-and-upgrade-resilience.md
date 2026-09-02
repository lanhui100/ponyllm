# Agent Note: 网关服务对外访问监听参数、API Token鉴权中间件与升级下载弹性

Status: implemented

## Problem

1. **外部服务监听不够灵活**：用户需要将 ponyllm 部署并对外提供服务（如 `0.0.0.0:8080`），但 `init` 向导写死了 `127.0.0.1:8080`，且 `ponyllm serve` 缺乏 `-a` (`--address`) 与 `-p` (`--port`) 等直观简写参数。
2. **缺乏入站请求安全鉴权**：当网关暴露给局域网或外部调用时，若无 API Key / Token 鉴权中间件，服务易被未授权滥用；同时需兼容 OpenAI 的 `Authorization: Bearer <key>` 与 Anthropic 的 `x-api-key: <key>` 两种请求头规范。
3. **跨地域网络下载 Release 资产脆弱性**：在部分网络环境下直接拉取 GitHub Release 资产可能发生偶发网络中断，需要重试与镜像加速容灾。

## Decision

1. **服务启动参数扩展与智能组装**：
   - `ponyllm serve` 增加 `-a, --address`（支持 `0.0.0.0` / `127.0.0.1` 等）和 `-p, --port`（支持任意端口），并与 `--bind` / 配置文件实现智能回退合并。
   - `ponyllm init` 向导增加监听地址选项（默认推荐 `0.0.0.0:8080` 外部访问，支持 `127.0.0.1:8080` 与自定义）及 API Token 设定。
2. **全协议双规范 API Token 鉴权中间件**：
   - 当 `gateway.api_key` 启用时，除 `/health` 外的所有路由均进行 Token 校验；
   - 提取 `Authorization: Bearer <token>` 或 `x-api-key: <token>`，匹配则放行，否则返回标准 HTTP 401 Unauthorized。
3. **升级下载多重重试与镜像加速**：
   - `upgrade` 下载资产时支持 3 次重试及镜像加速 fallback，提升升级成功率。

## Alternatives considered

- **强制要求所有环境都必须配置 API Key 且不可关闭**：
  - *否决理由*：本地开发环境或完全隔离内网可能需要免鉴权模式以简化调试。支持当 `api_key` 为空或 `"none"` 时自动跳过鉴权。

## Consequences

- 网关可安全、便捷地部署在各类服务器并对外提供 OpenAI 与 Anthropic 兼容的 AI 中转服务；
- 命令行参数交互更加符合运维与开发直觉。
