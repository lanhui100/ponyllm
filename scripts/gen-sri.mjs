#!/usr/bin/env node
// ==============================================================================
// gen-sri.mjs — build-time Subresource Integrity injection (Phase-3 task-8,
// VULN-20 转正)。在 `vite build` 之后运行：解析 web/dist/index.html，对本地
// `script[src]` 与 `link[rel=stylesheet][href]` 计算 sha384 并注入
// `integrity="sha384-…"`（缺 crossorigin 时一并补上），原地改写 index.html。
//
// 特性：
// - 零 npm 依赖（Node 内置 crypto/fs/path）；脚本位置即仓库根（web/dist 定位不依赖 cwd）。
// - 只处理本地相对/站内资产，跳过 http(s):、//、data: 及 favicon/icon（天然被
//   link[rel=stylesheet] 过滤）。
// - 已含 integrity 的标签不重复注入；标签已有 crossorigin 时不重复添加。
// - fail-fast：index.html 缺失、非 Vite 产物、或无可注入资产 → 非零退出，
//   防止无 SRI 的构建产物被发布。
// ==============================================================================
import { createHash } from 'node:crypto';
import { readFile, writeFile, stat, rename, unlink } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const DIST_DIR = process.env.WEB_DIST_DIR
  ? path.resolve(process.env.WEB_DIST_DIR)
  : path.join(REPO_ROOT, 'web', 'dist');
const INDEX_PATH = path.join(DIST_DIR, 'index.html');

const SRC_RE = /src\s*=\s*("([^"]*)"|'([^']*)')/i;
const HREF_RE = /href\s*=\s*("([^"]*)"|'([^']*)')/i;
const REL_RE = /rel\s*=\s*("([^"]*)"|'([^']*)')/i;
const INTEGRITY_RE = /\sintegrity\s*=/i;
const CROSSORIGIN_RE = /\bcrossorigin\b/i;
const TAG_RE = /<(script|link)\b[^>]*>/g;

/** 取匹配中引号捕获组的值 */
function attrValue(match) {
  return match ? (match[2] ?? match[3]) : null;
}

/** 是否本地（站内）资产 */
function isLocalUrl(raw) {
  if (!raw) return false;
  if (raw.startsWith('http://') || raw.startsWith('https://') || raw.startsWith('//') || raw.startsWith('data:')) {
    return false;
  }
  return true;
}

/** 站内 URL → dist 内绝对路径（去 query/hash、去前导 /） */
function localPath(raw) {
  const cleaned = decodeURIComponent(raw.split('?')[0].split('#')[0]);
  return path.resolve(DIST_DIR, cleaned.startsWith('/') ? cleaned.slice(1) : cleaned);
}

async function fileExists(abs) {
  try {
    const s = await stat(abs);
    return s.isFile();
  } catch {
    return false;
  }
}

function sha384(buf) {
  return 'sha384-' + createHash('sha384').update(buf).digest('base64');
}

/** 在标签闭合符前注入 integrity（必要时补 crossorigin），保留原标签其余语义 */
function inject(tag, integrity) {
  if (INTEGRITY_RE.test(tag)) return tag;
  const close = tag.match(/(\s*\/?>)$/);
  const closeStr = close ? close[1] : '>';
  const body = close ? tag.slice(0, -closeStr.length) : tag;
  const crossorigin = CROSSORIGIN_RE.test(tag) ? '' : ' crossorigin';
  return `${body} integrity="${integrity}"${crossorigin}${closeStr}`;
}

async function main() {
  let html;
  try {
    html = await readFile(INDEX_PATH, 'utf8');
  } catch {
    console.error(`gen-sri: FAIL — 找不到 ${INDEX_PATH}（先运行 vite build）`);
    process.exit(1);
  }
  if (!html.includes('<script') && !html.includes('<link')) {
    console.error(`gen-sri: FAIL — ${INDEX_PATH} 非 Vite 构建产物（无 script/link）`);
    process.exit(1);
  }

  const patches = new Map(); // 原标签文本 -> 替换文本
  for (const m of html.matchAll(TAG_RE)) {
    const tag = m[0];
    const isLink = m[1] === 'link';
    if (isLink) {
      const rel = attrValue(tag.match(REL_RE)) || '';
      if (!/\bstylesheet\b/i.test(rel)) continue;
    }
    const url = attrValue(isLink ? tag.match(HREF_RE) : tag.match(SRC_RE));
    if (!isLocalUrl(url)) continue;
    const abs = localPath(url);
    if (!(await fileExists(abs))) continue; // 弱引用资产不阻断，仍以 fail-fast 覆盖主场景

    const integrity = sha384(await readFile(abs));
    patches.set(tag, inject(tag, integrity));
  }

  if (patches.size === 0) {
    console.error(`gen-sri: FAIL — 未找到可注入的本地 script/link 资产（${INDEX_PATH}）`);
    process.exit(1);
  }

  for (const [tag, replacement] of patches) {
    html = html.split(tag).join(replacement);
  }

  // Phase-3b：原子写——同目录临时文件 + rename（同文件系统 rename 原子），
  // 进程中断也不会留下半写入的 index.html；失败时清理临时文件。
  const tmpPath = `${INDEX_PATH}.tmp-${process.pid}`;
  try {
    await writeFile(tmpPath, html);
    await rename(tmpPath, INDEX_PATH);
  } catch (err) {
    try {
      await unlink(tmpPath);
    } catch {
      /* 临时文件不存在则忽略 */
    }
    throw err;
  }
  console.log(`gen-sri: OK — ${patches.size} 个本地资产已注入 integrity → ${INDEX_PATH}`);
}

main().catch((err) => {
  console.error('gen-sri: FAIL —', err);
  process.exit(1);
});