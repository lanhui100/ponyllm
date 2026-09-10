<script setup lang="ts">
import { ref } from 'vue';
import type { CreateKeyResponse } from '../../types/admin';
import Icons from '../ui/Icons.vue';
import UiButton from '../ui/UiButton.vue';
import UiBadge from '../ui/UiBadge.vue';

const props = defineProps<{
  keyResult: CreateKeyResponse | null;
}>();

const emit = defineEmits<{
  (e: 'close'): void;
}>();

const copied = ref(false);

async function handleCopy() {
  if (!props.keyResult?.api_key) return;
  try {
    await navigator.clipboard.writeText(props.keyResult.api_key);
    copied.value = true;
    setTimeout(() => {
      copied.value = false;
    }, 2000);
  } catch {
    // Clipboard API fallback
  }
}
</script>

<template>
  <div v-if="keyResult" class="fixed inset-0 z-50 flex items-center justify-center p-4 bg-slate-900/40 backdrop-blur-md">
    <div class="bg-white/95 backdrop-blur-xl rounded-2xl shadow-2xl w-full max-w-lg overflow-hidden border border-slate-200/80">
      <div class="p-5 border-b border-slate-100 flex items-center justify-between">
        <h3 class="text-sm font-bold text-slate-900 flex items-center gap-2">
          <Icons name="key" size="16" class="text-amber-500" />
          API 密钥创建成功
        </h3>
        <UiBadge variant="success">仅展示一次</UiBadge>
      </div>

      <div class="p-5 space-y-4 text-sm">
        <div class="p-3 bg-amber-50/80 border border-amber-200/80 rounded-xl text-amber-800 leading-relaxed text-xs">
          <strong class="font-medium">安全提示：</strong>
          密钥明文仅在创建瞬间展示一次，网关后续仅回显脱敏掩码，且绝不持久化在浏览器中。请立即复制并妥善保管！
        </div>

        <div>
          <label class="block text-xs font-medium text-slate-600 mb-1.5">
            明文凭证 (ID: {{ keyResult.id }})
          </label>
          <div class="flex items-center gap-2">
            <input
              type="text"
              readonly
              :value="keyResult.api_key"
              class="flex-1 font-mono text-sm bg-slate-50 border border-slate-200 rounded-lg px-3 py-2 text-slate-900 select-all"
              data-testid="plaintext-key-input"
            />
            <UiButton
              variant="outline"
              size="sm"
              data-testid="copy-key-btn"
              @click="handleCopy"
            >
              <Icons :name="copied ? 'check' : 'copy'" size="13" />
              {{ copied ? '已复制' : '复制' }}
            </UiButton>
          </div>
        </div>

        <div class="flex items-center gap-4 text-xs text-slate-500 font-mono pt-1">
          <span>服务商: {{ keyResult.provider }}</span>
          <span>权重: {{ keyResult.weight }}</span>
          <span>优先级: {{ keyResult.priority }}</span>
        </div>
      </div>

      <div class="p-4 bg-slate-50 border-t border-slate-100 flex justify-end">
        <UiButton
          size="sm"
          data-testid="close-key-modal-btn"
          @click="emit('close')"
        >
          我已安全保存，关闭
        </UiButton>
      </div>
    </div>
  </div>
</template>
