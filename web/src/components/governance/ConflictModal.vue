<script setup lang="ts">
import Icons from '../ui/Icons.vue';
import UiButton from '../ui/UiButton.vue';

defineProps<{
  show: boolean;
}>();

const emit = defineEmits<{
  (e: 'refresh'): void;
  (e: 'close'): void;
}>();
</script>

<template>
  <div v-if="show" class="fixed inset-0 z-50 flex items-center justify-center p-4 bg-slate-900/40 backdrop-blur-xs">
    <div class="bg-white rounded-2xl shadow-xl w-full max-w-md overflow-hidden border border-slate-100">
      <div class="p-5 border-b border-slate-100 flex items-center justify-between">
        <h3 class="text-sm font-bold text-rose-600 flex items-center gap-2">
          <Icons name="lock" size="16" />
          配置版本冲突 (HTTP 412)
        </h3>
      </div>

      <div class="p-5 space-y-3 text-sm text-slate-600 leading-relaxed">
        <div class="p-3 bg-rose-50 border border-rose-200 rounded-xl text-rose-700 text-xs leading-relaxed">
          <strong>并发写入拦截：</strong>
          当前网关配置已被其他管理终端修改。为防止覆盖他人变更，提交已被安全拦截。
        </div>

        <p>
          请点击下方“拉取最新配置”获取服务端最新状态，核对无误后再次提交。
        </p>
      </div>

      <div class="p-4 bg-slate-50 border-t border-slate-100 flex items-center justify-end gap-2">
        <UiButton
          variant="ghost"
          size="sm"
          data-testid="close-conflict-modal-btn"
          @click="emit('close')"
        >
          暂不刷新
        </UiButton>
        <UiButton
          variant="destructive"
          size="sm"
          data-testid="refresh-config-btn"
          @click="emit('refresh')"
        >
          <Icons name="refresh" size="13" />
          拉取最新配置
        </UiButton>
      </div>
    </div>
  </div>
</template>
