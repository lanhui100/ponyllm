<script setup lang="ts">
import { h, ref, computed, watch, onMounted, onUnmounted } from 'vue';
import type { RecordedFrame } from '../types/telemetry';
import { scrubSecrets, generateCurlCommand } from '../utils/scrub';
import Icons from './ui/Icons.vue';
import UiTooltip from './ui/UiTooltip.vue';

const props = defineProps<{
  frame: RecordedFrame | null;
  isOpen: boolean;
}>();

const emit = defineEmits<{
  (e: 'close'): void;
}>();

const copiedCurl = ref(false);
const copiedError = ref(false);
const copiedResponse = ref(false);
const copiedRequest = ref(false);

// 模态切换：'json' 原始高亮 vs 'messages' 仅对话消息美观渲染
const reqMode = ref<'json' | 'messages'>('json');
const resMode = ref<'json' | 'messages'>('json');

// 1. 错误信息完全保留，不截取
const fullError = computed(() => {
  return props.frame?.error ? scrubSecrets(props.frame.error) : '';
});

// 2. Token 统计计算
const parsedResponseObj = computed(() => {
  if (!props.frame?.response_snippet) return null;
  try {
    return JSON.parse(props.frame.response_snippet);
  } catch {
    return null;
  }
});

const parsedUsageFromResponse = computed(() => {
  const res = parsedResponseObj.value;
  if (!res || typeof res !== 'object') return null;
  const usage = res.usage || res.response?.usage || res.message?.usage || res.usageMetadata;
  if (!usage) return null;

  let prompt = usage.prompt_tokens ?? usage.input_tokens ?? usage.promptTokenCount ?? null;
  let completion = usage.completion_tokens ?? usage.output_tokens ?? usage.candidatesTokenCount ?? null;
  let cached = usage.prompt_tokens_details?.cached_tokens ?? usage.cached_tokens ?? usage.prompt_cache_hit_tokens ?? usage.cache_read_input_tokens ?? usage.cachedContentTokenCount ?? 0;

  return {
    prompt: prompt !== null ? Number(prompt) : null,
    completion: completion !== null ? Number(completion) : null,
    cached: Number(cached),
  };
});

const ttftVal = computed(() => {
  if (props.frame?.ttft_ms !== undefined && props.frame.ttft_ms !== null) {
    return Math.round(props.frame.ttft_ms);
  }
  if (props.frame?.stream_flow?.ttft_ms !== undefined && props.frame.stream_flow.ttft_ms !== null) {
    return Math.round(props.frame.stream_flow.ttft_ms);
  }
  return null;
});

const promptTokens = computed(() => {
  if (props.frame?.prompt_tokens !== undefined && props.frame.prompt_tokens !== null) {
    return props.frame.prompt_tokens;
  }
  if (props.frame?.stream_flow?.prompt_tokens !== undefined && props.frame.stream_flow.prompt_tokens !== null) {
    return props.frame.stream_flow.prompt_tokens;
  }
  if (parsedUsageFromResponse.value?.prompt !== null && parsedUsageFromResponse.value?.prompt !== undefined) {
    return parsedUsageFromResponse.value.prompt;
  }
  return null;
});

const completionTokens = computed(() => {
  if (props.frame?.completion_tokens !== undefined && props.frame.completion_tokens !== null) {
    return props.frame.completion_tokens;
  }
  if (props.frame?.stream_flow?.completion_tokens !== undefined && props.frame.stream_flow.completion_tokens !== null) {
    return props.frame.stream_flow.completion_tokens;
  }
  if (parsedUsageFromResponse.value?.completion !== null && parsedUsageFromResponse.value?.completion !== undefined) {
    return parsedUsageFromResponse.value.completion;
  }
  return null;
});

const cachedTokens = computed(() => {
  if (props.frame?.cached_tokens !== undefined && props.frame.cached_tokens !== null) {
    return props.frame.cached_tokens;
  }
  if (props.frame?.stream_flow?.cached_tokens !== undefined && props.frame.stream_flow.cached_tokens !== null) {
    return props.frame.stream_flow.cached_tokens;
  }
  if (parsedUsageFromResponse.value?.cached !== null && parsedUsageFromResponse.value?.cached !== undefined) {
    return parsedUsageFromResponse.value.cached;
  }
  return 0;
});

const cacheHitRate = computed(() => {
  const p = promptTokens.value ?? 0;
  if (p <= 0) return 0;
  return Math.min(100, Math.round((cachedTokens.value / p) * 100));
});

// 3. 请求载荷脱敏 (不截断，但脱敏 sk-***)
const parsedRequest = computed(() => {
  if (!props.frame?.request_snippet) return null;
  try {
    return JSON.parse(props.frame.request_snippet);
  } catch {
    return null;
  }
});

const scrubbedRequestJson = computed(() => {
  if (!props.frame?.request_snippet) return null;
  const scrubbed = scrubSecrets(props.frame.request_snippet);
  try {
    return JSON.parse(scrubbed);
  } catch {
    return null;
  }
});

// 提取请求中的协议最新消息 (Latest Message)
const latestRequestMessage = computed(() => {
  const req = scrubbedRequestJson.value || parsedRequest.value;
  if (!req) {
    if (props.frame?.request_snippet) {
      return { role: 'user', content: scrubSecrets(props.frame.request_snippet) };
    }
    return null;
  }

  // 0. Antigravity CLI Envelope: 结构为 { project, requestId, request: { contents: [...], systemInstruction: {...} }, model }
  const innerReq = req.request && typeof req.request === 'object' ? req.request : req;

  // 1. Antigravity / Gemini contents (无论在根层级还是在 envelope.request 下)
  if (Array.isArray(innerReq.contents) && innerReq.contents.length > 0) {
    const last = innerReq.contents[innerReq.contents.length - 1];
    const role = last?.role === 'model' ? 'assistant' : (last?.role || 'user');
    let content = '';

    if (Array.isArray(last?.parts)) {
      content = last.parts
        .map((p: any) => {
          if (!p) return '';
          if (typeof p.text === 'string') return p.text;
          // Antigravity 函数调用 / 工具结果 / 代码执行
          if (p.functionCall) {
            return `\`\`\`json\n// 函数调用: ${p.functionCall.name}\n${JSON.stringify(p.functionCall.args || {}, null, 2)}\n\`\`\``;
          }
          if (p.functionResponse) {
            return `\`\`\`json\n// 工具返回 (${p.functionResponse.name}):\n${JSON.stringify(p.functionResponse.response || {}, null, 2)}\n\`\`\``;
          }
          if (p.inlineData) {
            return `[多模态内联数据: ${p.inlineData.mimeType || 'media'} (${p.inlineData.data?.length || 0} chars)]`;
          }
          return JSON.stringify(p, null, 2);
        })
        .filter(Boolean)
        .join('\n\n');
    } else if (typeof last?.parts === 'string') {
      content = last.parts;
    }

    if (content.trim()) {
      return {
        role,
        content: scrubSecrets(content),
      };
    }
  }

  // 2. OpenAI / Anthropic messages array
  if (Array.isArray(innerReq.messages) && innerReq.messages.length > 0) {
    const last = innerReq.messages[innerReq.messages.length - 1];
    let content = '';
    if (typeof last?.content === 'string') {
      content = last.content;
    } else if (Array.isArray(last?.content)) {
      content = last.content
        .map((c: any) => {
          if (typeof c === 'string') return c;
          if (c?.text) return c.text;
          if (c?.type === 'text') return c.text || '';
          return JSON.stringify(c, null, 2);
        })
        .join('\n');
    } else if (last?.content) {
      content = JSON.stringify(last.content, null, 2);
    }
    return {
      role: last?.role || 'user',
      content: scrubSecrets(content),
    };
  }

  // 3. Anthropic prompt / top-level prompt
  if (typeof innerReq.prompt === 'string') {
    return { role: 'user', content: scrubSecrets(innerReq.prompt) };
  }

  return null;
});

// 4. 响应信息 (保留原文，不脱敏，确保漂亮 json 渲染)
const cleanResponseSnippet = computed(() => {
  const raw = props.frame?.response_snippet;
  if (!raw) return '';
  if (raw === '[STREAM_STARTED]') {
    return '流传输已建立连接，正在传输中...';
  }
  return raw;
});

const responseJson = computed(() => {
  const raw = cleanResponseSnippet.value;
  if (!raw) return null;
  try {
    return JSON.parse(raw);
  } catch {
    return null;
  }
});

// 提取响应中的最新消息内容 (仅显示消息内容)
const latestResponseMessage = computed(() => {
  const res = responseJson.value;
  if (!res) {
    const raw = cleanResponseSnippet.value;
    if (raw && !raw.startsWith('[STREAM_COMPLETED')) {
      return { role: 'assistant', content: raw };
    }
    return null;
  }

  // 1. OpenAI choices
  if (Array.isArray(res.choices) && res.choices.length > 0) {
    const last = res.choices[res.choices.length - 1];
    if (last?.message?.content) {
      return { role: last.message.role || 'assistant', content: last.message.content };
    }
    if (last?.delta?.content) {
      return { role: 'assistant', content: last.delta.content };
    }
    if (last?.text) {
      return { role: 'assistant', content: last.text };
    }
  }

  // 2. Anthropic content array
  if (Array.isArray(res.content) && res.content.length > 0) {
    const texts = res.content
      .filter((item: any) => item?.type === 'text')
      .map((item: any) => item.text)
      .join('\n');
    if (texts) {
      return { role: res.role || 'assistant', content: texts };
    }
  }

  // 3. Antigravity candidates
  if (Array.isArray(res.candidates) && res.candidates.length > 0) {
    const cand = res.candidates[res.candidates.length - 1];
    const parts = cand?.content?.parts;
    if (Array.isArray(parts)) {
      const text = parts.map((p: any) => p?.text || '').join('');
      if (text) {
        return { role: 'assistant', content: text };
      }
    }
  }

  // 4. Fallback: stringify or snippet
  if (typeof res.output_text === 'string') {
    return { role: 'assistant', content: res.output_text };
  }

  return null;
});

// cURL 复现命令
const curlCommand = computed(() => {
  if (!props.frame) return '';
  const origin = typeof window !== 'undefined' ? window.location.origin : '';
  return generateCurlCommand(props.frame, origin);
});

async function copyCurl() {
  if (!curlCommand.value) return;
  try {
    await navigator.clipboard.writeText(curlCommand.value);
    copiedCurl.value = true;
    setTimeout(() => {
      copiedCurl.value = false;
    }, 2000);
  } catch {
    // fallback
  }
}

async function copyError() {
  if (!fullError.value) return;
  try {
    await navigator.clipboard.writeText(fullError.value);
    copiedError.value = true;
    setTimeout(() => {
      copiedError.value = false;
    }, 2000);
  } catch {
    // fallback
  }
}

async function copyResponseRaw() {
  const content = props.frame?.response_snippet || fullError.value;
  if (!content) return;
  try {
    await navigator.clipboard.writeText(content);
    copiedResponse.value = true;
    setTimeout(() => {
      copiedResponse.value = false;
    }, 2000);
  } catch {
    // fallback
  }
}

async function copyRequestRaw() {
  const content = scrubSecrets(props.frame?.request_snippet || '');
  if (!content) return;
  try {
    await navigator.clipboard.writeText(content);
    copiedRequest.value = true;
    setTimeout(() => {
      copiedRequest.value = false;
    }, 2000);
  } catch {
    // fallback
  }
}

function handleKeyDown(e: KeyboardEvent) {
  if (e.key === 'Escape' && props.isOpen) {
    emit('close');
  }
}

onMounted(() => {
  if (typeof window !== 'undefined') {
    window.addEventListener('keydown', handleKeyDown);
  }
});

onUnmounted(() => {
  if (typeof window !== 'undefined') {
    window.removeEventListener('keydown', handleKeyDown);
    if (typeof document !== 'undefined') {
      document.body.style.overflow = '';
    }
  }
});

watch(
  () => props.isOpen,
  (open) => {
    if (typeof document !== 'undefined') {
      document.body.style.overflow = open ? 'hidden' : '';
    }
  },
  { immediate: true }
);

// 简易极速轻量 Markdown 渲染器 (支持代码块、行内代码、标题、粗体、列表与换行)
function renderFastMarkdown(text: string): string {
  if (!text) return '';
  // 1. 转义 HTML 实体防止 XSS
  let escaped = text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;');

  // 2. 独立多行代码块 ```lang ... ```
  escaped = escaped.replace(/```([a-zA-Z0-9_-]*)\n([\s\S]*?)```/g, (_m, lang, code) => {
    const langBadge = lang ? `<div class="code-badge font-mono text-[10px] text-slate-400 select-none pb-1">${lang}</div>` : '';
    return `<div class="my-2.5 p-3 rounded-lg bg-slate-900 text-slate-100 font-mono text-xs overflow-x-auto custom-scrollbar">${langBadge}<pre class="leading-relaxed select-all"><code>${code.trim()}</code></pre></div>`;
  });

  // 3. 行内代码 `code`
  escaped = escaped.replace(/`([^`\n]+)`/g, '<code class="px-1.5 py-0.5 rounded bg-slate-200/80 font-mono text-xs text-indigo-700 select-all">$1</code>');

  // 4. 标题 # ## ###
  escaped = escaped.replace(/^### (.*$)/gim, '<h3 class="font-bold text-sm text-slate-900 mt-2 mb-1">$1</h3>');
  escaped = escaped.replace(/^## (.*$)/gim, '<h2 class="font-bold text-base text-slate-900 mt-3 mb-1.5">$1</h2>');
  escaped = escaped.replace(/^# (.*$)/gim, '<h1 class="font-bold text-lg text-slate-900 mt-3 mb-2">$1</h1>');

  // 5. 粗体与斜体
  escaped = escaped.replace(/\*\*([^*]+)\*\*/g, '<strong class="font-semibold text-slate-900">$1</strong>');
  escaped = escaped.replace(/\*([^*]+)\*/g, '<em class="italic">$1</em>');

  // 6. 列表项 - / *
  escaped = escaped.replace(/^\s*[-*]\s+(.*$)/gim, '<li class="ml-4 list-disc text-slate-800">$1</li>');

  // 7. 普通换行
  escaped = escaped.replace(/\n/g, '<br/>');

  return escaped;
}

// 可按层级折叠的递归漂亮 JSON 树状组件 (Collapsible JsonTree)
const JsonTree = {
  name: 'JsonTree',
  props: {
    data: { type: [Object, Array, String, Number, Boolean], default: null },
    depth: { type: Number, default: 0 },
    name: { type: [String, Number], default: '' },
  },
  setup(treeProps: { data: any; depth: number; name: string | number }) {
    // 默认前 2 层展开，深层长上下文默认可折叠
    const collapsed = ref(treeProps.depth >= 2);

    function toggle() {
      collapsed.value = !collapsed.value;
    }

    return () => {
      const { data, depth, name } = treeProps;
      const keyLabel = name !== '' ? h('span', { class: 'text-indigo-900 font-semibold font-mono text-[11px] shrink-0 mr-1.5 select-none' }, `"${name}":`) : null;

      if (data === null || data === undefined) {
        return h('div', { class: 'flex items-center' }, [keyLabel, h('span', { class: 'text-slate-400 italic' }, 'null')]);
      }
      if (typeof data === 'boolean') {
        return h('div', { class: 'flex items-center' }, [keyLabel, h('span', { class: 'text-amber-600 font-bold font-mono' }, String(data))]);
      }
      if (typeof data === 'number') {
        return h('div', { class: 'flex items-center' }, [keyLabel, h('span', { class: 'text-emerald-600 font-mono font-medium' }, String(data))]);
      }
      if (typeof data === 'string') {
        return h('div', { class: 'flex items-start' }, [
          keyLabel,
          h('span', { class: 'text-sky-800 whitespace-pre-wrap break-all leading-relaxed font-mono' }, `"${data}"`),
        ]);
      }

      if (Array.isArray(data)) {
        if (data.length === 0) {
          return h('div', { class: 'flex items-center' }, [keyLabel, h('span', { class: 'text-slate-400' }, '[]')]);
        }

        const foldToggle = h(
          'span',
          {
            class: 'inline-flex items-center justify-center w-3.5 h-3.5 mr-1 rounded hover:bg-slate-200/70 text-slate-400 hover:text-slate-700 cursor-pointer select-none text-[10px]',
            onClick: toggle,
          },
          collapsed.value ? '▶' : '▼'
        );

        if (collapsed.value) {
          return h('div', { class: 'flex items-center text-xs' }, [
            foldToggle,
            keyLabel,
            h(
              'span',
              {
                class: 'text-slate-400 hover:text-slate-600 cursor-pointer bg-slate-200/50 px-1.5 py-0.5 rounded font-mono text-[10px]',
                onClick: toggle,
              },
              `Array(${data.length}) [...]`
            ),
          ]);
        }

        return h('div', { class: 'space-y-0.5' }, [
          h('div', { class: 'flex items-center' }, [
            foldToggle,
            keyLabel,
            h('span', { class: 'text-slate-400 select-none text-[11px]' }, `[`),
          ]),
          h(
            'div',
            { class: 'pl-4 border-l-2 border-slate-200/70 space-y-1 my-0.5' },
            data.map((item, index) =>
              h(JsonTree, { key: index, data: item, depth: depth + 1, name: index })
            )
          ),
          h('div', { class: 'pl-4 text-slate-400 select-none text-[11px]' }, `]`),
        ]);
      }

      if (typeof data === 'object') {
        const keys = Object.keys(data);
        if (keys.length === 0) {
          return h('div', { class: 'flex items-center' }, [keyLabel, h('span', { class: 'text-slate-400' }, '{}')]);
        }

        const foldToggle = h(
          'span',
          {
            class: 'inline-flex items-center justify-center w-3.5 h-3.5 mr-1 rounded hover:bg-slate-200/70 text-slate-400 hover:text-slate-700 cursor-pointer select-none text-[10px]',
            onClick: toggle,
          },
          collapsed.value ? '▶' : '▼'
        );

        if (collapsed.value) {
          return h('div', { class: 'flex items-center text-xs' }, [
            foldToggle,
            keyLabel,
            h(
              'span',
              {
                class: 'text-slate-400 hover:text-slate-600 cursor-pointer bg-slate-200/50 px-1.5 py-0.5 rounded font-mono text-[10px]',
                onClick: toggle,
              },
              `{...${keys.length} keys}`
            ),
          ]);
        }

        return h('div', { class: 'space-y-0.5' }, [
          h('div', { class: 'flex items-center' }, [
            foldToggle,
            keyLabel,
            h('span', { class: 'text-slate-400 select-none text-[11px]' }, `{`),
          ]),
          h(
            'div',
            { class: 'pl-4 border-l-2 border-indigo-200/60 space-y-1 my-0.5' },
            keys.map((k) =>
              h(JsonTree, { key: k, data: data[k], depth: depth + 1, name: k })
            )
          ),
          h('div', { class: 'pl-4 text-slate-400 select-none text-[11px]' }, `}`),
        ]);
      }

      return h('span', {}, String(data));
    };
  },
};
</script>

<template>
  <div v-if="isOpen && frame" class="drawer-backdrop" @click="emit('close')">
    <div
      class="drawer-panel"
      role="dialog"
      aria-modal="true"
      aria-labelledby="drawer-title"
      @click.stop
    >
      <!-- 头部：标题与关闭 -->
      <div class="drawer-header flex items-center justify-between px-6 py-4 border-b border-slate-200/70 bg-white/80 backdrop-blur-md shrink-0">
        <div class="min-w-0 pr-4">
          <h2 id="drawer-title" class="text-lg font-bold text-slate-900 tracking-tight flex items-center gap-2">
            轨迹详情
          </h2>
          <div class="text-xs font-mono text-slate-500 truncate mt-0.5" :title="frame.request_id">
            ID: {{ frame.request_id }}
          </div>
        </div>
        <button
          class="text-slate-400 hover:text-slate-700 p-1.5 rounded-lg hover:bg-slate-100 transition-colors cursor-pointer"
          aria-label="关闭轨迹详情"
          @click="emit('close')"
        >
          <Icons name="cross" size="18" />
        </button>
      </div>

      <!-- 抽屉滚动正文 -->
      <div class="drawer-body flex-1 min-h-0 overflow-y-auto custom-scrollbar p-6 space-y-5">
        <!-- 元数据网格 -->
        <div class="swiss-card p-4 grid grid-cols-2 sm:grid-cols-3 gap-3.5 text-xs">
          <div>
            <span class="block text-slate-500 mb-1">状态码</span>
            <span
              class="badge inline-flex items-center px-2 py-0.5 rounded font-mono text-xs font-semibold"
              :class="(frame.status_code ?? 0) < 400 ? 'status-ok' : 'status-err'"
            >
              {{ frame.status_code ?? '--' }}
            </span>
          </div>

          <div>
            <span class="block text-slate-500 mb-1">总耗时</span>
            <span class="font-mono font-medium text-slate-800 text-sm">{{ frame.latency_ms }} ms</span>
          </div>

          <div>
            <span class="block text-slate-500 mb-1">Provider</span>
            <span class="inline-flex items-center px-2 py-0.5 rounded bg-slate-100 font-medium text-slate-800">
              {{ frame.provider || '--' }}
            </span>
          </div>

          <div class="col-span-2 sm:col-span-2">
            <span class="block text-slate-500 mb-1">端点</span>
            <span class="font-mono text-slate-800 break-all">{{ frame.endpoint }}</span>
          </div>

          <div>
            <span class="block text-slate-500 mb-1">Key 标识</span>
            <span class="font-mono font-medium text-slate-900 break-all">{{ frame.key_id }}</span>
          </div>

          <div class="col-span-2 sm:col-span-3">
            <span class="block text-slate-500 mb-1">时间戳</span>
            <span class="font-mono text-slate-600">{{ new Date(frame.timestamp).toLocaleString() }}</span>
          </div>
        </div>

        <!-- Token 与延迟指标强化展示卡片 -->
        <div class="swiss-card p-4 space-y-3">
          <div class="text-xs font-semibold text-slate-700 flex items-center justify-between">
            <span class="flex items-center gap-1.5">
              <Icons name="sparkles" size="14" class="text-amber-600" />
              Token 与延迟指标
            </span>
            <span v-if="ttftVal !== null" class="font-mono text-xs font-medium text-indigo-600 bg-indigo-50 px-2 py-0.5 rounded">
              TTFT: {{ ttftVal }} ms
            </span>
          </div>

          <div class="grid grid-cols-2 sm:grid-cols-4 gap-2.5 pt-1">
            <!-- 输入 Token -->
            <div class="p-2.5 rounded-lg bg-slate-50/90 border border-slate-200/60">
              <span class="text-[11px] text-slate-500 block">输入 Token</span>
              <span class="font-mono text-sm font-semibold text-slate-800 mt-0.5 block">
                {{ promptTokens !== null ? promptTokens.toLocaleString() : '--' }}
              </span>
            </div>

            <!-- 输出 Token -->
            <div class="p-2.5 rounded-lg bg-slate-50/90 border border-slate-200/60">
              <span class="text-[11px] text-slate-500 block">输出 Token</span>
              <span class="font-mono text-sm font-semibold text-slate-800 mt-0.5 block">
                {{ completionTokens !== null ? completionTokens.toLocaleString() : '--' }}
              </span>
            </div>

            <!-- 缓存命中数量 -->
            <div class="p-2.5 rounded-lg bg-slate-50/90 border border-slate-200/60">
              <span class="text-[11px] text-slate-500 block">缓存命中 Token</span>
              <span class="font-mono text-sm font-semibold text-emerald-700 mt-0.5 block">
                {{ cachedTokens.toLocaleString() }}
              </span>
            </div>

            <!-- 缓存命中率 -->
            <div class="p-2.5 rounded-lg bg-slate-50/90 border border-slate-200/60">
              <span class="text-[11px] text-slate-500 block">缓存命中率</span>
              <div class="flex items-center gap-1 mt-0.5">
                <span class="font-mono text-sm font-bold text-slate-900">
                  {{ cacheHitRate }}%
                </span>
                <span class="text-[10px] text-slate-400 font-mono">
                  ({{ cachedTokens }}/{{ promptTokens ?? 0 }})
                </span>
              </div>
            </div>
          </div>
        </div>

        <!-- 请求载荷模块 (可分层折叠漂亮的树状 JSON 渲染与极速 Markdown 渲染) -->
        <div v-if="frame.request_snippet" class="swiss-card p-4 space-y-3">
          <div class="flex items-center justify-between">
            <span class="text-xs font-semibold text-slate-800 flex items-center gap-1.5">
              <Icons name="file-text" size="14" class="text-slate-600" />
              请求载荷 (脱敏)
            </span>

            <div class="flex items-center gap-2">
              <UiTooltip :content="copiedRequest ? '已复制！' : '复制原始请求'">
                <button
                  type="button"
                  class="p-1 rounded-md text-slate-500 hover:text-slate-800 hover:bg-slate-100 transition-colors cursor-pointer"
                  aria-label="复制原始请求"
                  @click="copyRequestRaw"
                >
                  <Icons :name="copiedRequest ? 'check' : 'copy'" size="13" />
                </button>
              </UiTooltip>

              <!-- Switch 模态切换按钮 -->
              <div class="segment-track inline-flex items-center text-xs">
                <button
                  type="button"
                  class="px-2.5 py-1 rounded-md transition-all cursor-pointer font-medium"
                  :class="reqMode === 'json' ? 'bg-white shadow-2xs text-slate-900 font-semibold' : 'text-slate-500 hover:text-slate-800'"
                  @click="reqMode = 'json'"
                >
                  JSON 原始
                </button>
                <button
                  type="button"
                  class="px-2.5 py-1 rounded-md transition-all cursor-pointer font-medium"
                  :class="reqMode === 'messages' ? 'bg-white shadow-2xs text-slate-900 font-semibold' : 'text-slate-500 hover:text-slate-800'"
                  @click="reqMode = 'messages'"
                >
                  对话消息
                </button>
              </div>
            </div>
          </div>

          <!-- JSON 树状分层渲染：支持层级折叠展开、无边框底色，使用细窄美观滚动条 -->
          <div
            v-if="reqMode === 'json'"
            class="bg-slate-50/70 p-4 rounded-xl max-h-[460px] overflow-y-auto custom-scrollbar font-mono text-xs select-all"
          >
            <JsonTree v-if="scrubbedRequestJson" :data="scrubbedRequestJson" />
            <pre v-else class="whitespace-pre-wrap break-all text-slate-800 leading-relaxed">{{ scrubSecrets(frame.request_snippet) }}</pre>
          </div>

          <!-- 对话消息：按照协议最新的消息呈现，支持简易极速 Markdown 渲染 -->
          <div v-else class="space-y-2">
            <div v-if="latestRequestMessage" class="p-4 rounded-xl bg-blue-50/70 text-xs leading-relaxed space-y-2 max-h-[460px] overflow-y-auto custom-scrollbar">
              <div class="flex items-center justify-between text-[11px] font-mono font-bold uppercase tracking-wider text-blue-800 border-b border-blue-200/60 pb-1.5">
                <span>最新请求消息 ({{ latestRequestMessage.role }})</span>
              </div>
              <!-- Markdown 渲染区域 -->
              <div
                class="font-sans break-words text-blue-950 leading-relaxed text-sm"
                v-html="renderFastMarkdown(latestRequestMessage.content)"
              />
            </div>
            <div v-else class="p-4 text-center text-xs text-slate-400">
              无解析出的请求消息文本
            </div>
          </div>
        </div>

        <!-- 响应内容模块 (含错误异常统一呈现，不截取，层级折叠漂亮 JSON 与简易 Markdown 渲染) -->
        <div v-if="frame.response_snippet || fullError" class="swiss-card p-4 space-y-3">
          <div class="flex items-center justify-between">
            <span class="text-xs font-semibold text-slate-800 flex items-center gap-1.5">
              <Icons :name="fullError ? 'warning' : 'sparkles'" size="14" :class="fullError ? 'text-rose-600' : 'text-emerald-600'" />
              响应内容 {{ fullError ? '(异常详情)' : '(保留原文)' }}
            </span>

            <div class="flex items-center gap-2">
              <UiTooltip :content="copiedResponse ? '已复制！' : '复制响应内容'">
                <button
                  type="button"
                  class="p-1 rounded-md text-slate-500 hover:text-slate-800 hover:bg-slate-100 transition-colors cursor-pointer"
                  aria-label="复制响应内容"
                  @click="copyResponseRaw"
                >
                  <Icons :name="copiedResponse ? 'check' : 'copy'" size="13" />
                </button>
              </UiTooltip>

              <!-- Switch 模态切换按钮 -->
              <div class="segment-track inline-flex items-center text-xs">
                <button
                  type="button"
                  class="px-2.5 py-1 rounded-md transition-all cursor-pointer font-medium"
                  :class="resMode === 'json' ? 'bg-white shadow-2xs text-slate-900 font-semibold' : 'text-slate-500 hover:text-slate-800'"
                  @click="resMode = 'json'"
                >
                  JSON 原始
                </button>
                <button
                  type="button"
                  class="px-2.5 py-1 rounded-md transition-all cursor-pointer font-medium"
                  :class="resMode === 'messages' ? 'bg-white shadow-2xs text-slate-900 font-semibold' : 'text-slate-500 hover:text-slate-800'"
                  @click="resMode = 'messages'"
                >
                  消息内容
                </button>
              </div>
            </div>
          </div>

          <!-- JSON 树状分层渲染：支持层级折叠展开、无边框底色，使用细窄美观滚动条 -->
          <div
            v-if="resMode === 'json'"
            class="bg-slate-50/70 p-4 rounded-xl max-h-[460px] overflow-y-auto custom-scrollbar font-mono text-xs select-all"
            :class="{ '!bg-rose-50/60 text-rose-950': Boolean(fullError) }"
          >
            <div v-if="fullError" class="mb-3 p-3 rounded-lg bg-rose-100/60 text-rose-900 font-sans text-xs whitespace-pre-wrap break-all leading-relaxed">
              <strong>错误详情: </strong>{{ fullError }}
            </div>
            <JsonTree v-if="responseJson" :data="responseJson" />
            <pre v-else class="whitespace-pre-wrap break-all text-slate-800 leading-relaxed">{{ cleanResponseSnippet }}</pre>
          </div>

          <!-- 仅显示最新消息内容，支持简易极速 Markdown 渲染 -->
          <div v-else class="space-y-2">
            <div
              v-if="fullError"
              class="p-4 rounded-xl bg-rose-50/80 text-rose-950 text-xs leading-relaxed space-y-2 max-h-[460px] overflow-y-auto custom-scrollbar"
            >
              <div class="font-mono font-bold text-[11px] uppercase tracking-wider text-rose-800 border-b border-rose-200/60 pb-1">
                错误异常详情
              </div>
              <div
                class="font-sans break-all text-sm leading-relaxed"
                v-html="renderFastMarkdown(fullError)"
              />
            </div>

            <div
              v-else-if="latestResponseMessage"
              class="p-4 rounded-xl bg-emerald-50/60 text-emerald-950 text-xs leading-relaxed space-y-2 max-h-[460px] overflow-y-auto custom-scrollbar"
            >
              <div class="flex items-center justify-between text-[11px] font-mono font-bold uppercase tracking-wider text-emerald-800 border-b border-emerald-200/60 pb-1">
                <span>{{ latestResponseMessage.role }}</span>
              </div>
              <!-- Markdown 渲染区域 -->
              <div
                class="font-sans break-words text-sm leading-relaxed text-slate-900"
                v-html="renderFastMarkdown(latestResponseMessage.content)"
              />
            </div>

            <div v-else class="p-4 text-center text-xs text-slate-400">
              无解析出的回复消息内容
            </div>
          </div>
        </div>

        <!-- cURL 复现命令 -->
        <div class="swiss-card p-4 space-y-2.5">
          <div class="flex items-center justify-between">
            <span class="text-xs font-semibold text-slate-800">cURL 复现命令</span>
            <button
              type="button"
              class="inline-flex items-center gap-1 text-xs px-2.5 py-1 rounded-md bg-white hover:bg-slate-50 border border-slate-200 font-medium text-slate-700 shadow-2xs cursor-pointer transition-colors"
              @click="copyCurl"
            >
              <Icons :name="copiedCurl ? 'check' : 'copy'" size="13" />
              {{ copiedCurl ? '已复制 ✓' : '复制命令' }}
            </button>
          </div>
          <pre class="code-block text-xs font-mono text-slate-800 bg-slate-900/5 p-3 rounded-lg overflow-x-auto whitespace-pre select-all">{{ curlCommand }}</pre>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.drawer-backdrop {
  position: fixed;
  top: 0;
  left: 0;
  width: 100vw;
  height: 100vh;
  background: rgba(15, 23, 42, 0.35);
  backdrop-filter: blur(6px);
  -webkit-backdrop-filter: blur(6px);
  z-index: 999;
  display: flex;
  justify-content: flex-end;
}

.drawer-panel {
  width: 680px;
  max-width: 95vw;
  height: 100vh;
  background: rgba(255, 255, 255, 0.94);
  backdrop-filter: blur(20px);
  -webkit-backdrop-filter: blur(20px);
  border-left: 1px solid rgba(226, 232, 240, 0.9);
  box-shadow: -10px 0 35px -5px rgba(15, 23, 42, 0.1);
  display: flex;
  flex-direction: column;
}

.badge {
  letter-spacing: 0.02em;
}

.status-ok {
  background: rgba(220, 252, 231, 0.9);
  color: #166534;
  border: 1px solid #bbf7d0;
}

.status-err {
  background: rgba(254, 226, 226, 0.9);
  color: #991b1b;
  border: 1px solid #fecaca;
}
</style>
