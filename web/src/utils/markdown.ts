import DOMPurify from 'dompurify';

/**
 * 简易极速轻量 Markdown 渲染器（F12 安全加固版）。
 *
 * 安全管线（顺序即安全保证，勿重排）：
 * 1. 原始不可信输入先经 DOMPurify 消毒——剥离存活危险标签（script/img/svg/
 *    iframe/object/...）与事件属性，处理实体双解（`&lt;script&gt;` → 文本）；
 *    DOMPurify 在"含标签输入"下会解码实体，故必须先消毒原始输入，
 *    否则转义后的 `&lt;b oncopy=...&gt;` 会在消毒时被重新解析成活元素。
 * 2. 对消毒结果转义 `& < > " '`（补充 `"` 与 `'`，防属性注入）后做 markdown
 *    token 化——只插入本模块白名单字面量标签，不再经过任何消毒/解码。
 *
 * 由 FrameDrawer.vue 复用的唯一渲染实现（契约 F12：渲染器抽 utils 便于单测）。
 */
export function renderFastMarkdown(text: string): string {
  if (!text) return '';

  // 1. 原始输入 DOMPurify 消毒（剥危险标签/事件属性，中立化实体双解）
  const sanitized = DOMPurify.sanitize(text);

  // 2. 转义 HTML 实体与引号防止 XSS（先 & 后其它，避免二次转义）
  let escaped = sanitized
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');

  // 3. 独立多行代码块 ```lang ... ```
  escaped = escaped.replace(/```([a-zA-Z0-9_-]*)\n([\s\S]*?)```/g, (_m, lang, code) => {
    const langBadge = lang ? `<div class="code-badge font-mono text-[10px] text-slate-400 select-none pb-1">${lang}</div>` : '';
    return `<div class="my-2.5 p-3 rounded-lg bg-slate-900 text-slate-100 font-mono text-xs overflow-x-auto custom-scrollbar">${langBadge}<pre class="leading-relaxed select-all"><code>${code.trim()}</code></pre></div>`;
  });

  // 4. 行内代码 `code`
  escaped = escaped.replace(/`([^`\n]+)`/g, '<code class="px-1.5 py-0.5 rounded bg-slate-200/80 font-mono text-xs text-indigo-700 select-all">$1</code>');

  // 5. 标题 # ## ###
  escaped = escaped.replace(/^### (.*$)/gim, '<h3 class="font-bold text-sm text-slate-900 mt-2 mb-1">$1</h3>');
  escaped = escaped.replace(/^## (.*$)/gim, '<h2 class="font-bold text-base text-slate-900 mt-3 mb-1.5">$1</h2>');
  escaped = escaped.replace(/^# (.*$)/gim, '<h1 class="font-bold text-lg text-slate-900 mt-3 mb-2">$1</h1>');

  // 6. 粗体与斜体
  escaped = escaped.replace(/\*\*([^*]+)\*\*/g, '<strong class="font-semibold text-slate-900">$1</strong>');
  escaped = escaped.replace(/\*([^*]+)\*/g, '<em class="italic">$1</em>');

  // 7. 列表项 - / *
  escaped = escaped.replace(/^\s*[-*]\s+(.*$)/gim, '<li class="ml-4 list-disc text-slate-800">$1</li>');

  // 8. 普通换行
  escaped = escaped.replace(/\n/g, '<br/>');

  return escaped;
}
