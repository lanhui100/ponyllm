# 安全漏洞修复启动提示词

复制以下内容到新会话中粘贴，即可开始修复（也可直接把本文件内容作为首条消息）：

---

请基于 ponyllm 仓库根目录下的 `SECURITY-REPORT.md`（tokens.ponyjob.top 多维度安全检测最终报告）启动安全漏洞修复。

【背景】该报告由 agent team 对生产站点 https://tokens.ponyjob.top 做被动/只读检测后汇总，全部为真实证据。当前仓库即该站点的后端/网关代码（crates/ponyllm-server 等），修复应以代码修复 + 部署配置修复为主。

【修复范围与优先级】
- P0：① 源站 IP 直接暴露无 CDN/WAF（High）—— 检查 deploy/ 目录的部署配置，给出接入 CDN/WAF、源站限制回源 IP 的方案并落地可执行配置；② 管理凭据存 sessionStorage + CSP 含 'unsafe-inline'（Medium）—— 前端（Vite/Vue SPA 入口与 serve 端）改为服务端会话或收紧 CSP 为 nonce/hash，禁止内联脚本。
- P1：③ 非 SNI 请求返回 Traefik 默认自签证书（指纹泄露，返回 421 或配置正式兜底证书）；④ token 经 `?token=/?key=` URL 参数传递的风险（改一次性握手码或 fragment）；⑤ 管理 API 写路径（/api/admin/keys、gateway-keys 签发/吊销等）暴露于公开 JS bundle 的攻击面（管理面分离或至少强化审计与轮换）；⑥ OAuth redirect_uri 服务端白名单校验缺失（需持有效凭据补测并加校验）。
- P2：⑦ 404 兜底响应统一注入安全头（CSP/HSTS/XFO）；⑧ API 增加速率限制与失败锁定（429）；⑨ 开启 OCSP stapling；⑩ 修复项完成后重跑检测验证。

【工作方式】
1. 先读 SECURITY-REPORT.md，再按 P0→P2 顺序逐项制定修复计划（可用 dev-orchestrator skill 拆解任务）。
2. 每项修复给出：问题定位（代码/配置位置）→ 修复方案 → 落地修改 → 验证方式。
3. 禁止为验证引入新的破坏性操作；验证以本地单元测试、构建通过、以及只读 curl 复测线上安全头/端点为准。
4. 全部完成后输出修复清单（每项：状态/证据/残留风险），并把未修复项标为 TODO 写入 backlog。
