// @vitest-environment happy-dom
// Phase-2 安全修复验收（F12 / VULN-06 v-html 消毒）。
//
// 红相要求：渲染器须从 FrameDrawer.vue 抽取为 `web/src/utils/markdown.ts`
// （契约矩阵 F12："渲染器抽 utils 便于单测"）。HEAD 上该模块不存在 →
// 本文件 import 解析失败 = F12 未实现的直接证据。
//
// ## 接口契约（Executor 按此实现）
// ```ts
// // web/src/utils/markdown.ts
// export function renderFastMarkdown(text: string): string
// ```
// 要求：
// 1. 先转义 `& < > " '`（补充 `"` 与 `'`），再做 markdown token 化；
// 2. 输出经 DOMPurify 消毒：任何存活标签/事件属性被剥离；
// 3. FrameDrawer.vue 复用同一实现（删除组件内私有副本）。
//
// 断言以 DOM 语义为准（DOMParser 解析后检查存活元素/事件属性），避免对
// "被转义成惰性文本"的攻击串做字符串误判。

import { describe, it, expect } from 'vitest';
import { renderFastMarkdown } from './markdown';

// 载荷矩阵（含绕过向量）
const bypassVectors = [
  '<script>alert(1)</script>',
  '<img src=x onerror=alert(1)>',
  '<svg/onload=alert(1)>',
  '<iframe src="https://evil.example"></iframe>',
  '<object data="https://evil.example"></object>',
  'javascript:alert(1)',
  '" onmouseover="alert(1)"',
  "' onfocus='alert(1)'",
  '&lt;script&gt;alert(1)&lt;/script&gt;', // 实体双解绕过
  '```js\n<img src=x onerror=alert(1)>\n```', // 代码块内载荷
  '**<b oncopy=alert(1)>**', // 格式化内嵌标签
  '`<img src=x onerror=alert(1)>`', // 行内代码内载荷
];

function liveDangerousTags(html: string): Element[] {
  const doc = new DOMParser().parseFromString(html, 'text/html');
  return Array.from(doc.querySelectorAll('script,img,svg,iframe,object,embed,link,meta,form'));
}

function liveEventAttrs(html: string): Attr[] {
  const doc = new DOMParser().parseFromString(html, 'text/html');
  return Array.from(doc.querySelectorAll('*'))
    .flatMap((el) => Array.from(el.attributes))
    .filter((a) => /^on/i.test(a.name) || a.name === 'href' && a.value.trim().toLowerCase().startsWith('javascript:'));
}

describe('F12 v-html 消毒（renderFastMarkdown）', () => {
  for (const payload of bypassVectors) {
    it(`neutralizes: ${payload.slice(0, 48)}`, () => {
      const html = renderFastMarkdown(payload);
      const live = liveDangerousTags(html);
      const attrs = liveEventAttrs(html);
      expect(live, `存活危险标签: ${JSON.stringify(live.map((e) => e.outerHTML))}`).toHaveLength(0);
      expect(attrs, `存活事件/javascript 属性: ${JSON.stringify(attrs.map((a) => `${a.name}=${a.value}`))}`).toHaveLength(0);
    });
  }

  it('转义双引号与单引号（F12 补 " 和 \'）', () => {
    const html = renderFastMarkdown('say "hi" and \'yo\'');
    expect(html).toContain('&quot;');
    expect(html).not.toContain('"hi"');
    expect(html).not.toContain("'yo'");
  });

  it('保留良性 markdown 格式化（粗体/行内代码/标题）', () => {
    const html = renderFastMarkdown('**bold** and `code`\n# Title');
    expect(html).toContain('<strong');
    expect(html).toContain('<code');
    expect(html).toContain('<h1');
  });

  it('空输入返回空串', () => {
    expect(renderFastMarkdown('')).toBe('');
    expect(renderFastMarkdown(null as unknown as string)).toBe('');
  });
});
