# Agent Note: 修复 gemini-3.1-flash-image 图像编辑 (/v1/images/edits) 忽略原图与凭空生图缺陷

Status: proposed

## Problem

使用 OpenAI 图像协议调用 ponyllm 网关的 `/v1/images/edits` 端点（底层使用 Antigravity `gemini-3.1-flash-image`）时，模型没有基于用户提交的原图进行修改，而是凭空生成全新图片。

根本原因分析：
1. **原图非空硬门禁缺失**：`handle_image_edits` 与 `run_images_request` 未对 `image` 字段进行硬性校验。当客户端 multipart/form-data 上传格式有差异、字段名未对齐或缺失图片时，`image` 静默为 `None`，后端未拦截并静默降级为文本生图（返回 200）。
2. **多模态图生图意图丢失（Prompt 未锚定原图）**：Gemini 多模态生成接口中，直接放置 `[inlineData, prompt]` 时，对于短指令（如 "make it winter"、"add glasses"），模型易直接将文本解析为独立 text-to-image 提示词。官方实践及主流封装均需在多模态 parts 中对指令进行基底图像引用锚定。
3. **强制注入 aspectRatio 破坏构图**：若客户端或 SDK 默认带 `size: 1024x1024`，网关强行注入 `imageConfig.aspectRatio = "1:1"`，当原图非 1:1 时会导致模型重采样或舍弃原图构图。
4. **Session 隔离混淆**：`extract_or_generate_session_id` 仅基于 prompt 生成哈希，不同图片但相同 prompt 会共享同一个 sessionId 产生会话污染。

## Proposal

1. **协议层与路由层入口校验（Fail-Fast）**：
   - `/v1/images/edits` 强制校验 `image` 必须存在且有效；若未提供图片或解析失败，立即返回 `400 BAD_REQUEST` (`invalid_input`, `"image is required for image editing"`）。
2. **提示词上下文锚定**：
   - 在 `images_to_antigravity_request` 中，当 `image_part.is_some()` 时，将 prompt 规范化锚定为显式编辑指令：
     `"Based on the provided input image, edit and modify it according to the following instruction: {prompt}"`（若原提示词已显式包含 "based on the provided image" 或 "edit this image" 则不重复包裹）。
3. **编辑模式纵横比（Aspect Ratio）策略调优**：
   - 在图生图编辑模式（`image_part.is_some()`）下，除非显式指定与原图不同的变换，否则不主动向 `generationConfig` 强加 `aspectRatio`，保留原图的原始纵横比。
4. **Session 因子扩展**：
   - 当携带图片时，将图片的哈希/摘要纳入 sessionId 计算，确保每次不同图片编辑的上下文独立。

## Alternatives considered

- **要求客户端自行在 prompt 补充 "edit this image"**：被否决。OpenAI SDK 的标准用例是直接传修改动作（如 `a cute baby sea otter wearing a beret`），破坏了 OpenAI 协议兼容性，网关应负责协议对齐与模型适配。
- **完全丢弃 edits 里的 size 字段**：对于用户明确想要改变比例的场景可能有用，但大多数 SDK（如 Python openai）如果不传默认也会发送 size，因此在 edits 模式下，默认不强设 aspectRatio 更符合保留原图长宽比的直觉。

## Consequences

- 彻底杜绝空图请求伪装成成功生图的问题。
- 极大提升 `gemini-3.1-flash-image` 在 OpenAI 协议下的图生图与图像编辑效果，稳定基于原图修改。
- 编写端到端单元测试与集成测试，确保回归安全。
