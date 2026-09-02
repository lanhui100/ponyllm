# Agent Note: 自动高熵网关 API Key 生成与鉴权凭据管理命令

Status: implemented

## Problem

1. **初始鉴权凭据过于简单或未强制初始化**：此前默认使用简单的 `sk-ponyllm-local` 或空字符，若网关暴露给局域网或外部网络，存在凭证过于简单被爆破或未授权访问的风险。
2. **缺乏便捷修改与生成 API Key 的 CLI 指令**：用户需要修改或重置网关 API Token 时，需手动编辑 `ponyllm.toml` 文本文件，缺乏类似 `ponyllm auth` 的直观命令行操作。
3. **凭据可见性不足**：每次生成或更新 Key 后，需要有醒目的控制台屏幕展示，方便用户一键复制到各大 AI 客户端（如 Cursor、Claude Code、Continue、SDK）中。

## Decision

1. **高熵安全 API Key 算法**：
   - 生成算法：`sk-pony-` + 32位加密级随机 UUIDv4 Hex（如 `sk-pony-8f2e4c1a...`），杜绝简单字符。
2. **服务启动保底自生成与醒目展示**：
   - `ponyllm serve` 启动时若未配置或未提供 Key，自动生成高熵安全 Key，并自动同步至配置文件与控制台 Banner，确保零配置启动即具备安全防护。
3. **增加 `auth` 与 `key gateway` 命令**：
   - 支持 `ponyllm auth [KEY]` 与 `ponyllm key gateway [KEY]`：
     - 若指定 `KEY`，更新网关凭证并持久化至 `ponyllm.toml`；
     - 若留空未指定 `KEY`，自动生成高熵安全 Key 并持久化；
     - 两种情况均在控制台屏幕以高亮边框打印完整的 API Key 与使用指南。

## Alternatives considered

- **仅在 `ponyllm.toml` 中手动编辑，不提供专用 CLI 命令**：
  - *否决理由*：命令行即席生成/重置鉴权凭证是云原生与网关工具的标准体验，能极大提升用户体验并减少手动语法错误。

## Consequences

- 网关默认安全等级全面提升，杜绝简单凭据；
- 用户可通过简单命令快速生成、修改并复制 API Key。
