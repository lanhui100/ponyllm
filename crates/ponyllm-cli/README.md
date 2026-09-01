# ponyllm-cli

## 模块职责
命令行接口与交互式终端控制台。

## 契约与功能
- `ponyllm init`: 交互式配置初始化向导；
- `ponyllm provider`: 模型提供商 CRUD 管理；
- `ponyllm key`: Key 账户池管理与拨测；
- `ponyllm model`: 默认映射模型管理；
- `ponyllm serve`: 启动 HTTP/SSE 网关；
- `ponyllm tui`: 启动全屏 Ratatui 交互式监控看板；
- `ponyllm status / telemetry`: 遥测与黑匣子录波查看。
