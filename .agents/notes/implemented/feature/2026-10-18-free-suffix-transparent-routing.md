# Agent Note: 裸名路由对 `-free` / `:free` 免费档后缀透明

Status: implemented

## Problem

网关按裸模型名（不指定 provider）路由时只做**精确同名**匹配：`resolve_pinned_targets`
阶段 1 在全部 provider 的 `default_model ∪ models` 中找与请求名逐字相等的模型。但免费档上游
发布的模型名带免费后缀——opencode-zen 形态 `glm-5.3-flash-free`、OpenRouter 形态
`glm-5.3-flash:free`——于是：

- 客户端按文档/习惯请求 `glm-5.3-flash`，而配置里只有 `glm-5.3-flash-free` 时，精确匹配
  落空；请求随后掉进关键字启发式（阶段 3），把**不存在的付费物理模型名**（如
  `deepseek-v4.1-flash`）发给上游，或直接报 `No provider configured`。
- 反向同理：请求 `glm-5.3-flash-free`、配置里只有 `glm-5.3-flash` 时无法回落。
- `opencode-zen` 每上一个免费模型就要在 `canonicalize_model_name` 里手写一条别名
  （`laya-free` 已是既有补丁），属于逐模型硬编码，不可持续。

已有契约（`2026-10-16-model-surface-provider-qualified-and-weighted-routing`）只覆盖
"裸名匹配所有同名模型、按 priority 依次倒换"，未覆盖免费档后缀。

## Decision

1. **纯函数入 core**：`crates/ponyllm-core/src/endpoints.rs` 新增
   `strip_free_suffix(name)`（剥离尾部 `-free` / `:free`，ASCII 大小写不敏感，
   `&str` 零拷贝、UTF-8 边界安全）与 `same_model_modulo_free_suffix(a, b)`，
   并由 `ponyllm_core` 顶层 re-export。两者均有 truth table 单测。
2. **路由新增阶段 1c（仅裸名）**：`resolve_pinned_targets` 在精确同名（阶段 1/1b）之后新增
   免费后缀透明匹配：
   - 请求名含 `/`（显式 provider pin）时**不参与**，保持既有 provider 作用域；
   - 只比较 `strip_free_suffix` 后的基名（大小写不敏感），配置侧同样剥离后再比；
   - **正向**（请求基名 → 配置的 `-free`/`:free` 变体）作为**兜底候选追加**，精确同名候选
     始终排在其前（即使变体 price/latency 评分更好，也不得抢占用户显式请求的模型）；
   - **反向**（请求 `x-free` → 配置的基名 `x`）只在**没有任何精确匹配**时启用，避免把
     显式请求的免费模型静默换成付费同名模型；
   - 命中变体后跳过阶段 3 关键字启发式，防止对未配置的付费物理名下注；
   - 变体模型自身的 `spec.fallbacks` 一并纳入二级候选链（4b 种子从
     `candidates ∪ free_variant_candidates` 收集）。
3. **文档**：README 路由提示补充该语义（正向兜底 + 反向回落 + `-free`/`:free` 等价）。
4. **机器门禁**：`cargo test -p ponyllm-server --test free_suffix_routing_tests`
   覆盖正向/反向/冒号等价/精确优先/provider pin 隔离/非 free 后缀不误配 7 条断言。

## Alternatives considered

- **继续在 `canonicalize_model_name` 里逐模型加别名**：每接一个免费模型就要改一次 core
  硬编码并随上游改名漂移，且只能表达"免费名 → 付费名"单向映射；否决为通用方案。
- **任意前缀/后缀匹配（不止 free）**：请求 `gpt-4o-mini` 会命中配置的 `gpt-4o`
  （物理模型被静默降级），`gemini-3.8-flash` 会命中 `-high/-medium/-low` 四个内部变体；
  破坏"未知模型名透传上游"的既有语义，风险大于收益；否决，范围严格限定免费后缀。
- **双向都只在精确缺失时匹配**：请求 `x` 时配置的 `x-free` 永不参与，免费档只能靠显式
  请求名使用，失去"付费不可用时免费兜底"的价值；否决。
- **变体与精确候选混排后统一按 priority 排序**：免费模型优先级/评分更高时会抢走用户显式
  请求的模型，语义倒退；否决，改为分段排序（精确段 + 变体段）。
- **在 `ParsedRequestModel::parse` 里把 `:free` 当标签剥离或归一为 `-free`**：`:` 同时是
  Ollama 真实模型 tag（`llama3:70b`）与既有策略/档位标签分隔符，剥离会改写请求语义、
  影响解析契约与 thinking/strategy 覆盖；否决，解析层零改动，只在路由比较时等价。
- **改 `list_all_models` / `/v1/models` 暴露面**：免费变体本就以 `provider/model` 形式列出，
  本次只解决入站裸名解析；否决，避免动已冻结的 B003 暴露面契约。

## Consequences

- 裸名请求新增一类兜底候选：`glm-5.3-flash`（精确，付费）→ `glm-5.3-flash-free`
  （变体，免费）；某 provider 不可用/额度耗尽时免费档可接住，无需客户端改模型名。
- 已有契约不回归：`auto` 虚拟路由、`provider/model` 显式 pin、`/v1/models` 暴露面、
  同名多 provider 的 priority 倒换、Token 白名单校验（按请求名，非物理名）均未改动。
- 唯一行为收紧：请求 `x` 且只配置 `x-free` 时不再落入关键字启发式发不存在的物理名，
  而是解析到 `x-free`；请求 `x-free` 且同时配置 `x-free`/`x` 时仍只走 `x-free`。
- 验证：`cargo test -p ponyllm-core --lib endpoints`（6 passed）、
  `cargo test -p ponyllm-server --test free_suffix_routing_tests`（7 passed）、
  `gateway_tests` / `request_routing_tests` / `model_protocol_precedence_tests` /
  `acceptance_b003_model_capacity_tests` 全绿（共 75 passed）。
- 已知边界（靠 review）：`skills/ponyllm-add-model/SKILL.md` 的免费模型接入流程未变，
  它只写配置；本记录只改入站路由解析。
