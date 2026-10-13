// Wave 1 红相测试：前端打包资源保留（Assets Retention / Version Buffering）
//
// 契约目标：
// 在多节点滚动升级期间，不同节点 Pod 可能交替提供服务。如果新版本构建时彻底清空
// dist 目录（emptyOutDir: true），或者生产打包镜像未保留历史版本的静态 assets 文件，
// 则访问旧节点 HTML 的客户端在向新节点请求旧 hash 的 js/css 时会直接报 404。
//
// 契约断言：
// 1. web/vite.config.ts 中的 build.emptyOutDir 不得为 true（建议为 false 或受控分层），
//    确保多次打包或构建产物支持保留历史 assets 文件缓冲。
// 2. 模拟构建输出场景：当 dist/assets 存在前一版本的历史文件时，新构建必须保留该历史文件。

import { describe, it, expect } from 'vitest';
import * as fs from 'node:fs';
import * as path from 'node:path';

describe('Web Build Assets Retention & Version Buffering (Wave 1 Red)', () => {
  it('vite.config.ts 中的 build 配置不得启用 emptyOutDir: true', () => {
    const configPath = path.resolve(__dirname, 'vite.config.ts');
    const content = fs.readFileSync(configPath, 'utf-8');

    // 红相断言：当前 vite.config.ts 设置了 emptyOutDir: true
    // 方案要求 emptyOutDir: false 或移除清空，使得产物具备版本缓冲
    const hasEmptyOutDirTrue = /emptyOutDir\s*:\s*true/.test(content);

    expect(
      hasEmptyOutDirTrue,
      'vite.config.ts build.emptyOutDir 仍为 true，滚动发布期间会导致旧 assets 被擦除产生 404 (Wave 1 红相确证)',
    ).toBe(false);
  });
});
