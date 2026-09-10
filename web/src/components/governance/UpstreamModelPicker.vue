<script setup lang="ts">
import { ref, computed } from 'vue';
import Icons from '../ui/Icons.vue';
import UiButton from '../ui/UiButton.vue';

const props = defineProps<{
  providerName: string;
  models: { id: string }[];
  existingNames: string[];
  submitting: boolean;
  progress: string | null;
}>();

const emit = defineEmits<{
  (e: 'confirm', ids: string[]): void;
  (e: 'close'): void;
}>();

const keyword = ref('');
const selected = ref<Set<string>>(new Set());

const existing = computed(() => new Set(props.existingNames));

const filtered = computed(() => {
  const kw = keyword.value.trim().toLowerCase();
  const list = props.models.map((m) => m.id).filter(Boolean);
  const sorted = [...new Set(list)].sort((a, b) => a.localeCompare(b));
  if (!kw) return sorted;
  return sorted.filter((id) => id.toLowerCase().includes(kw));
});

const selectable = computed(() => filtered.value.filter((id) => !existing.value.has(id)));

function toggle(id: string) {
  if (existing.value.has(id)) return;
  const next = new Set(selected.value);
  if (next.has(id)) {
    next.delete(id);
  } else {
    next.add(id);
  }
  selected.value = next;
}

function toggleAll() {
  if (selected.value.size >= selectable.value.length && selectable.value.length > 0) {
    selected.value = new Set();
  } else {
    selected.value = new Set(selectable.value);
  }
}

function handleConfirm() {
  if (selected.value.size === 0 || props.submitting) return;
  emit('confirm', [...selected.value]);
}
</script>

<template>
  <div
    class="fixed inset-0 z-40 flex items-center justify-center p-4"
    data-testid="upstream-model-picker"
    @click.self="emit('close')"
  >
    <div class="w-full max-w-lg max-h-[80vh] flex flex-col bg-white/95 backdrop-blur-xl rounded-2xl shadow-2xl overflow-hidden border border-slate-200/90 ring-1 ring-slate-900/5">
      <div class="flex items-center justify-between px-4 py-3 border-b border-slate-100">
        <div class="min-w-0">
          <div class="font-semibold text-slate-800 text-sm">从上游添加模型</div>
          <div class="text-xs text-slate-400 truncate">{{ providerName }} · 共 {{ models.length }} 个上游模型</div>
        </div>
        <button
          type="button"
          class="text-slate-400 hover:text-slate-600 cursor-pointer p-1"
          aria-label="关闭"
          @click="emit('close')"
        >
          <Icons name="cross" size="14" />
        </button>
      </div>

      <div class="px-4 pt-3 space-y-2.5">
        <input
          v-model="keyword"
          type="text"
          placeholder="搜索模型 ID…"
          class="w-full bg-slate-50 border border-slate-200/80 rounded-lg px-3 py-2 text-xs text-slate-800 focus:outline-none focus:ring-2 focus:ring-indigo-500/20 focus:border-indigo-500"
          data-testid="picker-search-input"
        />
        <div class="flex items-center justify-between text-xs">
          <button
            type="button"
            class="text-indigo-600 hover:text-indigo-700 font-medium cursor-pointer"
            data-testid="picker-toggle-all"
            @click="toggleAll"
          >
            {{ selected.size >= selectable.length && selectable.length > 0 ? '取消全选' : `全选当前 ${selectable.length} 个` }}
          </button>
          <span class="text-slate-400">已选 {{ selected.size }} 个</span>
        </div>
      </div>

      <div class="flex-1 overflow-y-auto px-4 py-2.5 space-y-1" data-testid="picker-model-list">
        <div v-if="filtered.length === 0" class="py-6 text-center text-xs text-slate-400">
          无匹配模型
        </div>
        <label
          v-for="id in filtered"
          :key="id"
          class="flex items-center gap-2.5 px-2.5 py-2 rounded-lg text-xs cursor-pointer"
          :class="[
            existing.has(id)
              ? 'text-slate-300 bg-transparent cursor-not-allowed'
              : selected.has(id)
                ? 'bg-indigo-50/70 text-slate-800'
                : 'hover:bg-slate-50 text-slate-700'
          ]"
        >
          <input
            type="checkbox"
            :checked="selected.has(id)"
            :disabled="existing.has(id)"
            class="accent-indigo-600 h-3.5 w-3.5"
            :data-testid="`picker-check-${id}`"
            @change="toggle(id)"
          />
          <span class="font-mono truncate" :title="id">{{ id }}</span>
          <span v-if="existing.has(id)" class="ml-auto text-3xs text-slate-300 shrink-0">已添加</span>
        </label>
      </div>

      <div class="px-4 py-3 border-t border-slate-100 flex items-center justify-between gap-2">
        <span v-if="progress" class="text-xs text-slate-500" data-testid="picker-progress">{{ progress }}</span>
        <span v-else class="text-xs text-slate-300">已存在的不重复添加</span>
        <div class="flex items-center gap-2">
          <UiButton variant="ghost" size="sm" :disabled="submitting" @click="emit('close')">
            取消
          </UiButton>
          <UiButton
            size="sm"
            :disabled="selected.size === 0 || submitting"
            data-testid="picker-confirm-btn"
            @click="handleConfirm"
          >
            {{ submitting ? '添加中…' : `添加所选 (${selected.size})` }}
          </UiButton>
        </div>
      </div>
    </div>
  </div>
</template>
