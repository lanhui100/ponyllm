# Agent Note: 新增 ponyllm-add-model skill（生产服务加模型）

Status: implemented

## Decision

新增 `skills/ponyllm-add-model/SKILL.md` + `install.sh`（现在时）：把 space-bunny-free 接入与 OpenRouter 镜像清理两次实战沉淀为可复用 skill。内容按 `.agents/skills/README.md` 五要素组织——触发式 description、前置与真相源清单、五步程序（上游 models 目录核对 → 字段映射表 → Admin API 写入 → 验证 → 下架双证据）、正反成对校准样例（space-bunny 实形 + fledge protocol 教训 + 三条拒绝样例）、验证与报告格式。

关键规则全部来自实测先例：zen 新 free 模型 protocol 默认 `chat`（fledge `ModelProtocolUnsupported` 教训）；`max_output` 按上游天花板填（safeguard 只 floor 不扩容）；`pricing_mode` 小写（`Uniform` 报 400）；CUD 必带 `If-Match`（缺失/过期 412）；下架需目录无条目 + 直调 404 双证据。

## Alternatives considered

- 只写操作手册式文档（无触发头/验收测试）：落选。按技能契约，无触发式 description 的文档在懒加载路由时撞不到；无验收测试则退化不可验证。
- skill 直改 secret 文件：落选。生产真相源是 `ponyllm-live-config` + 网关热加载；直改文件绕过 `config_version` 乐观并发与审计链，且 skill 明示不碰 key 原文。
- 拆加模型/下架为两个 skill：落选。下架是加模型的逆操作，共用同一套字段映射与双证据判定；Rule of three 下当前复用频率只够养一个 skill。

## Consequences

- 机械可查：`pytest tests/acceptance/add_model_skill_test.py` 16 passed；dry-run 核对现网 `opencode-zen/space-bunny-free` 六字段与 skill 示例一致、零差异；安全对抗 PASS（无 key 原文/无鉴权绕过/删操作有门禁）；可用性首轮 FAIL（步骤 1 `<UPSTREAM_BASE>` 无具体来源命令，新人无法独立解析）→ 已修复（拆 1a `GET /api/admin/providers` 取 base_url + 1b 拼 `/models`），复验 16 绿；另代修 `install.sh` 可执行位 600→755（与现网两 skill 的 755 对齐）。
