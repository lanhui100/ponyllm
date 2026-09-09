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
          网关配置刚被改过，提交没成功
        </h3>
      </div>

      <div class="p-5 space-y-3 text-sm text-slate-600 leading-relaxed">
        <div class="p-3 bg-rose-50 border border-rose-200 rounded-xl text-rose-700 text-xs leading-relaxed">
          你操作的同时，网关配置发生了变化——通常是系统在后台自动续期了密钥，
          也可能是另一个管理页面做了修改。你填的内容都还在，不用重填。
        </div>

        <p>
          点“刷新数据”拿到最新配置后，再点一次保存即可。
        </p>
      </div>

      <div class="p-4 bg-slate-50 border-t border-slate-100 flex items-center justify-end gap-2">
        <UiButton
          variant="ghost"
          size="sm"
          data-testid="close-conflict-modal-btn"
          @click="emit('close')"
        >
          知道了
        </UiButton>
        <UiButton
          variant="destructive"
          size="sm"
          data-testid="refresh-config-btn"
          @click="emit('refresh')"
        >
          <Icons name="refresh" size="13" />
          刷新数据
        </UiButton>
      </div>
    </div>
  </div>
</template>
