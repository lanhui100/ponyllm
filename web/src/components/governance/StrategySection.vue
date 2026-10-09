<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import Icons from '../ui/Icons.vue';
import { toast } from '../../composables/useToast';
import UiBadge from '../ui/UiBadge.vue';
import UiButton from '../ui/UiButton.vue';
import type { ModelView } from '../../types/admin';

const props = defineProps<{
  adminWriteEnabled: boolean;
  autoModels?: string[];
  /** 全量模型目录（来自 useAdminConfig.models），用于「添加候选」弹窗的复选列表。 */
  allModels?: ModelView[];
}>();

const emit = defineEmits<{
  (e: 'updateAutoModels', models: string[]): Promise<void>;
}>();

const localAutoModels = ref<string[]>([...(props.autoModels || [])]);
const savingAutoModels = ref(false);

/** 拖拽/上移下移的写权限闸门：只读控制台一律冻结。 */
const canReorder = computed(() => props.adminWriteEnabled);

/** 「添加候选」弹窗开关。 */
const pickerOpen = ref(false);
/** 勾选次序（`${provider}/${name}` 标识数组）：每次勾选 push 到末尾，取消勾选按值移除。
    不用 Set —— 渲染时需要稳定的数组顺序语义，且每次勾选的「次序」就是这里的下标。 */
const checkedModelIds = ref<string[]>([]);

/** 拖拽状态：null 表示当前没有进行中的拖拽（浏览器在行外发起的 dragstart 会保持 null）。 */
const dragFromIndex = ref<number | null>(null);
const dragOverIndex = ref<number | null>(null);

watch(
  () => props.autoModels,
  (val) => {
    if (val) localAutoModels.value = [...val];
  },
  { deep: true }
);

/** 目录项主键：`${provider}/${name}`；provider 缺省时留空前缀，保证标识唯一且可解析。 */
function modelKey(m: { provider?: string; name: string }): string {
  return `${m.provider ?? ''}/${m.name}`;
}

/** 弹窗候选项：按目录原始顺序呈现（不排序，勾选次序由 checkedModelIds 决定）。 */
const pickerOptions = computed(() =>
  (props.allModels || []).map((m) => ({
    id: modelKey(m),
    name: m.name,
    provider: m.provider || '',
    alreadyConfigured: localAutoModels.value.includes(m.name),
  })),
);

/** 主键 -> 模型名，用于「确认添加」时把勾选次序还原成模型名（不靠字符串切割，避免模型名含 '/'）。 */
const modelNameByKey = computed(() => {
  const map = new Map<string, string>();
  for (const m of props.allModels || []) map.set(modelKey(m), m.name);
  return map;
});

function moveUp(index: number) {
  if (!props.adminWriteEnabled || index <= 0) return;
  const list = [...localAutoModels.value];
  const item = list.splice(index, 1)[0];
  list.splice(index - 1, 0, item);
  localAutoModels.value = list;
}

function moveDown(index: number) {
  if (!props.adminWriteEnabled || index >= localAutoModels.value.length - 1) return;
  const list = [...localAutoModels.value];
  const item = list.splice(index, 1)[0];
  list.splice(index + 1, 0, item);
  localAutoModels.value = list;
}

function removeModel(index: number) {
  if (!props.adminWriteEnabled) return;
  const list = [...localAutoModels.value];
  list.splice(index, 1);
  localAutoModels.value = list;
}

function onDragStart(index: number) {
  if (!props.adminWriteEnabled) return;
  dragFromIndex.value = index;
}

function onDragOver(index: number) {
  if (dragFromIndex.value === null) return;
  dragOverIndex.value = index;
}

function onDrop(index: number) {
  const from = dragFromIndex.value;
  resetDrag();
  if (from === null || !props.adminWriteEnabled || from === index) return;
  const list = [...localAutoModels.value];
  const item = list.splice(from, 1)[0];
  list.splice(index, 0, item);
  localAutoModels.value = list;
}

function onDragEnd() {
  resetDrag();
}

function resetDrag() {
  dragFromIndex.value = null;
  dragOverIndex.value = null;
}

/** 打开弹窗：每次打开都重置勾选状态，绝不残留上一次的选择。 */
function openPicker() {
  if (!props.adminWriteEnabled) return;
  checkedModelIds.value = [];
  pickerOpen.value = true;
}

function cancelPicker() {
  pickerOpen.value = false;
  checkedModelIds.value = [];
}

/** 勾选切换：勾选记录「次序」，取消勾选按值移除（重复勾选不重复记录）。 */
function toggleChecked(id: string, checked: boolean) {
  if (checked) {
    if (!checkedModelIds.value.includes(id)) checkedModelIds.value.push(id);
    return;
  }
  checkedModelIds.value = checkedModelIds.value.filter((x) => x !== id);
}

function isChecked(id: string): boolean {
  return checkedModelIds.value.includes(id);
}

/** 确认添加：按**勾选次序**追加到末尾，跳过已存在的项；不触发自动保存。 */
function confirmPicker() {
  const list = [...localAutoModels.value];
  const seen = new Set(list);
  let appended = 0;
  for (const id of checkedModelIds.value) {
    const name = modelNameByKey.value.get(id);
    if (!name || seen.has(name)) continue;
    seen.add(name);
    list.push(name);
    appended += 1;
  }
  localAutoModels.value = list;
  pickerOpen.value = false;
  checkedModelIds.value = [];
  if (appended > 0) {
    toast.success(`已添加 ${appended} 个候选模型，记得保存优先级配置`);
  }
}

async function handleSaveAutoModels() {
  if (!props.adminWriteEnabled || savingAutoModels.value) return;
  savingAutoModels.value = true;
  try {
    await emit('updateAutoModels', localAutoModels.value);
    toast.success('Auto 候选模型优先级排序已保存生效');
  } catch (err: unknown) {
    toast.error(`保存 Auto 模型失败: ${err instanceof Error ? err.message : String(err)}`);
  } finally {
    savingAutoModels.value = false;
  }
}
</script>

<template>
  <div class="space-y-6">
    <!-- Auto 智能路由：候选模型优先级排序（Auto 已接管全局调度，此处即唯一配置面） -->
    <div class="swiss-card p-6">
      <div class="flex items-center justify-between mb-4">
        <div>
          <h2 class="text-base font-bold text-slate-900 flex items-center gap-2">
            <Icons name="sparkles" size="18" class="text-emerald-600" />
            Auto智能路由
          </h2>
          <p class="text-xs text-slate-500 mt-1">
            下游 Agent 调用纯 <code class="px-1 py-0.5 bg-slate-100 rounded text-slate-800 font-mono">auto</code> 时的选型规则：首选免费主力，次选收费主力；顺序从上到下逐级备选与故障熔断
          </p>
        </div>
        <div class="flex items-center gap-2">
          <UiButton
            variant="default"
            size="sm"
            data-testid="auto-model-save"
            :disabled="!adminWriteEnabled || savingAutoModels"
            @click="handleSaveAutoModels"
          >
            <Icons v-if="savingAutoModels" name="refresh" size="14" class="animate-spin mr-1" />
            <Icons v-else name="check" size="14" class="mr-1" />
            保存优先级配置
          </UiButton>
        </div>
      </div>

      <!-- 候选模型顺序编辑列表 -->
      <div class="space-y-2">
        <div class="flex items-center justify-between text-xs font-medium text-slate-600 px-1">
          <span>Auto路由模型顺序</span>
          <span>共 {{ localAutoModels.length }} 个配置项</span>
        </div>

        <div v-if="localAutoModels.length === 0" class="text-center py-6 border border-dashed rounded-xl text-slate-400 text-xs">
          暂无自定义配置，使用系统默认顺序 (gemini-3.8-flash -> deepseek-v4-flash)
        </div>

        <div
          v-for="(model, index) in localAutoModels"
          :key="model"
          data-testid="auto-model-row"
          :data-model="model"
          :data-draggable-disabled="canReorder ? 'false' : 'true'"
          :draggable="canReorder ? 'true' : 'false'"
          class="flex items-center justify-between p-1.5 bg-white/70 border border-slate-200/80 rounded-lg hover:bg-white transition-all shadow-2xs"
          :class="[
            canReorder ? 'cursor-grab active:cursor-grabbing' : 'cursor-default opacity-60',
            dragOverIndex === index ? 'ring-2 ring-indigo-400 border-indigo-300' : '',
          ]"
          @dragstart="onDragStart(index)"
          @dragover.prevent="onDragOver(index)"
          @drop.prevent="onDrop(index)"
          @dragend="onDragEnd"
        >
          <div class="flex items-center gap-2 min-w-0">
            <span aria-hidden="true" class="text-slate-300 text-[11px] leading-none select-none">⋮⋮</span>
            <span class="w-5 h-5 rounded-full bg-slate-100 flex items-center justify-center text-[10px] font-bold text-slate-600 shrink-0">
              {{ index + 1 }}
            </span>
            <span class="font-mono text-xs font-medium text-slate-900 truncate">{{ model }}</span>
            <UiBadge v-if="index === 0" variant="default" class="text-[10px] scale-90" data-testid="auto-model-top-badge">
              最高优
            </UiBadge>
          </div>

          <div class="flex items-center gap-1">
            <UiButton
              variant="ghost"
              size="sm"
              class="h-7 w-7 p-0"
              data-testid="auto-model-up"
              :disabled="index === 0 || !adminWriteEnabled"
              title="上移"
              :aria-label="`上移 ${model}`"
              @click="moveUp(index)"
            >
              <span class="text-xs font-bold">↑</span>
            </UiButton>
            <UiButton
              variant="ghost"
              size="sm"
              class="h-7 w-7 p-0"
              data-testid="auto-model-down"
              :disabled="index === localAutoModels.length - 1 || !adminWriteEnabled"
              title="下移"
              :aria-label="`下移 ${model}`"
              @click="moveDown(index)"
            >
              <span class="text-xs font-bold">↓</span>
            </UiButton>
            <UiButton
              variant="ghost"
              size="sm"
              class="h-7 w-7 p-0 text-red-500 hover:text-red-700 hover:bg-red-50"
              :data-testid="`auto-model-remove-${index}`"
              :disabled="!adminWriteEnabled"
              title="移除"
              :aria-label="`移除 ${model}`"
              @click="removeModel(index)"
            >
              <Icons name="trash" size="14" />
            </UiButton>
          </div>
        </div>

        <!-- 添加候选：弹窗勾选全量模型，按勾选次序追加（无输入框） -->
        <div v-if="adminWriteEnabled" class="flex gap-2 pt-2">
          <UiButton
            variant="secondary"
            size="sm"
            data-testid="auto-model-add-open"
            @click="openPicker"
          >
            <Icons name="plus" size="14" class="mr-1" />
            添加候选
          </UiButton>
        </div>
      </div>
    </div>

    <!-- 添加候选弹窗：全模型复选列表，勾选次序即加入次序 -->
    <div
      v-if="pickerOpen"
      class="fixed inset-0 z-50 flex items-center justify-center p-4 bg-slate-900/40 backdrop-blur-md"
      data-testid="auto-model-picker"
      @click.self="cancelPicker"
    >
      <div class="bg-white/95 backdrop-blur-xl rounded-2xl shadow-2xl w-full max-w-lg overflow-hidden border border-slate-200/80">
        <div class="p-5 border-b border-slate-100 flex items-center justify-between">
          <h3 class="text-sm font-bold text-slate-900 flex items-center gap-2">
            <Icons name="plus" size="16" class="text-emerald-600" />
            添加候选模型
          </h3>
          <span class="text-xs text-slate-500">按勾选次序追加到列表末尾</span>
        </div>

        <div class="p-4 max-h-96 overflow-y-auto space-y-1">
          <div
            v-for="opt in pickerOptions"
            :key="opt.id"
            data-testid="auto-model-option"
            :data-model-id="opt.id"
            class="flex items-center gap-2.5 p-2 rounded-lg border border-transparent hover:bg-slate-50 cursor-pointer"
            :class="isChecked(opt.id) ? 'bg-slate-50 border-slate-200' : ''"
          >
            <input
              type="checkbox"
              class="rounded border-slate-300 text-slate-900 focus:ring-0 cursor-pointer shrink-0"
              :checked="isChecked(opt.id)"
              @change="toggleChecked(opt.id, ($event.target as HTMLInputElement).checked)"
            />
            <span class="font-mono text-xs font-medium text-slate-900 truncate">{{ opt.name }}</span>
            <span class="text-[10px] text-slate-400 shrink-0">{{ opt.provider }}</span>
            <UiBadge v-if="opt.alreadyConfigured" variant="secondary" class="text-[10px] scale-90 ml-auto shrink-0">
              已配置
            </UiBadge>
          </div>

          <div v-if="pickerOptions.length === 0" class="text-center py-6 border border-dashed rounded-xl text-slate-400 text-xs">
            模型目录为空，请先在「服务商」中挂载模型
          </div>
        </div>

        <div class="p-4 bg-slate-50 border-t border-slate-100 flex items-center justify-between gap-2">
          <span class="text-xs text-slate-500">已勾选 {{ checkedModelIds.length }} 个</span>
          <div class="flex items-center gap-2">
            <UiButton
              variant="ghost"
              size="sm"
              data-testid="auto-model-picker-cancel"
              @click="cancelPicker"
            >
              取消
            </UiButton>
            <UiButton
              size="sm"
              data-testid="auto-model-picker-confirm"
              @click="confirmPicker"
            >
              确认添加
            </UiButton>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>