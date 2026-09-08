<script setup lang="ts">
import Icons from './ui/Icons.vue';
import UiButton from './ui/UiButton.vue';

defineProps<{
  health: 'ok' | 'down' | 'degraded' | 'unknown';
  transport: 'sse' | 'polling' | 'offline';
  isDown: boolean;
}>();

const emit = defineEmits<{
  (e: 'retry'): void;
}>();
</script>

<template>
  <div
    class="flex items-center justify-between px-4 py-2.5 rounded-xl shadow-xs mb-6 transition-all duration-200"
    :class="isDown ? 'bg-rose-50 border border-rose-200' : 'bg-white'"
  >
    <div class="flex items-center gap-2.5 text-xs">
      <!-- 极简脉冲呼吸灯 -->
      <span class="relative flex h-2.5 w-2.5 items-center justify-center">
        <span
          v-if="health === 'ok'"
          class="animate-ping absolute inline-flex h-full w-full rounded-full bg-emerald-400 opacity-75"
        />
        <span
          class="relative inline-flex rounded-full h-2 w-2"
          :class="{
            'bg-emerald-500': health === 'ok',
            'bg-amber-500': health === 'degraded',
            'bg-rose-500': health === 'down',
            'bg-slate-400': health === 'unknown',
          }"
        />
      </span>

      <span class="text-slate-700">
        网关状态: <strong class="font-semibold text-slate-900">{{ health.toUpperCase() }}</strong>
      </span>

      <span class="text-slate-300">·</span>

      <span
        class="px-2 py-0.5 rounded-full text-2xs font-medium"
        :class="{
          'bg-emerald-50 text-emerald-700': transport === 'sse',
          'bg-blue-50 text-blue-700': transport === 'polling',
          'bg-rose-50 text-rose-700': transport === 'offline',
        }"
      >
        {{ transport === 'sse' ? '实时 SSE' : transport === 'polling' ? '轮询 (1.5s)' : '离线' }}
      </span>
    </div>

    <div v-if="isDown" class="flex items-center gap-2">
      <span class="text-xs text-rose-600 font-medium">连接异常</span>
      <UiButton
        variant="destructive"
        size="sm"
        @click="emit('retry')"
      >
        <Icons name="refresh" size="12" />
        重试连接
      </UiButton>
    </div>
  </div>
</template>
