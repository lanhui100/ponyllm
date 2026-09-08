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
    class="flex items-center justify-between px-4.5 py-3 rounded-xl mb-6 transition-all duration-200"
    :class="isDown ? 'bg-rose-50/90 text-rose-800' : 'borderless-card'"
  >
    <div class="flex items-center gap-3 text-sm">
      <!-- 极简冷翠玉脉冲呼吸灯 -->
      <span class="relative flex h-2.5 w-2.5 items-center justify-center">
        <span
          v-if="health === 'ok'"
          class="animate-ping absolute inline-flex h-full w-full rounded-full bg-teal-400 opacity-75"
        />
        <span
          class="relative inline-flex rounded-full h-2 w-2"
          :class="{
            'bg-teal-500': health === 'ok',
            'bg-amber-500': health === 'degraded',
            'bg-rose-500': health === 'down',
            'bg-slate-400': health === 'unknown',
          }"
        />
      </span>

      <span class="text-slate-700 font-medium">
        网关状态: <strong class="font-bold text-slate-900">{{ health.toUpperCase() }}</strong>
      </span>

      <span class="text-slate-300">·</span>

      <span
        class="px-2.5 py-0.5 rounded-full text-xs font-medium"
        :class="{
          'bg-teal-50/90 text-teal-800': transport === 'sse',
          'bg-indigo-50/90 text-indigo-700': transport === 'polling',
          'bg-rose-50/90 text-rose-700': transport === 'offline',
        }"
      >
        {{ transport === 'sse' ? '实时流 (SSE)' : transport === 'polling' ? '轮询中 (1.5s)' : '服务离线' }}
      </span>
    </div>

    <div v-if="isDown" class="flex items-center gap-2">
      <span class="text-xs text-rose-600 font-medium">连接异常</span>
      <UiButton
        variant="destructive"
        size="sm"
        @click="emit('retry')"
      >
        <Icons name="refresh" size="13" />
        重试连接
      </UiButton>
    </div>
  </div>
</template>
