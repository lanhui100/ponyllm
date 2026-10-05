// @vitest-environment happy-dom
// Phase-2 安全修复验收（F13 / VULN-16 __PONY_BASE__ runtime 覆写）。
//
// 红相要求：HEAD 上 `resolveBaseURL()` 读取 `window.__PONY_BASE__` runtime 覆写
// （alova.ts:74-77），攻击者注入的页面脚本可重定向控制台 API 基址到恶意服务。
// 修复后改为构建期 `import.meta.env.VITE_API_BASE` 注入、剥离 runtime 覆写。
// HEAD 上本用例断言 `resolveBaseURL()` 不返回注入值 → 失败（红相）。
//
// 实现契约：`resolveBaseURL()` 返回值不得包含 `window.__PONY_BASE__` 设定的值。

import { describe, it, expect, afterEach } from 'vitest';
import { resolveBaseURL } from './alova';

describe('F13 __PONY_BASE__ runtime override removed', () => {
  afterEach(() => {
    const w = window as unknown as Record<string, unknown>;
    delete w.__PONY_BASE__;
  });

  it('ignores a hostile window.__PONY_BASE__ override', () => {
    const w = window as unknown as Record<string, unknown>;
    w.__PONY_BASE__ = 'https://evil.example/base/';
    const base = resolveBaseURL();
    expect(base).not.toContain('evil.example');
    // 且不得等于注入值本身（无论是否裁剪尾部斜杠）
    expect(base).not.toBe('https://evil.example/base');
    expect(base).not.toBe('https://evil.example/base/');
  });
});
