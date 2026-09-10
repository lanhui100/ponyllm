<script setup lang="ts">
import { computed } from 'vue';
import { useToast, type ToastType } from '../../composables/useToast';
import Icons from './Icons.vue';
import UiButton from './UiButton.vue';

const props = withDefaults(
  defineProps<{
    // 兼容原有单条 message 属性传参
    message?: string | null;
    type?: ToastType;
  }>(),
  {
    message: null,
    type: 'info',
  }
);

const { currentToast, dismissToast, handleConfirmAction } = useToast();

// 活动的通知项，优先取全局状态，降级取 props.message
const activeItem = computed(() => {
  if (currentToast.value) {
    return currentToast.value;
  }
  if (props.message) {
    return {
      id: 'prop-toast',
      type: props.type || 'info',
      message: props.message,
      closable: true,
      isConfirm: false,
    };
  }
  return null;
});

const isConfirm = computed(() => activeItem.value?.isConfirm === true);

const typeConfig = computed(() => {
  const type = activeItem.value?.type || 'info';
  switch (type) {
    case 'success':
      return {
        icon: 'check' as const,
        iconWrap: 'text-emerald-700',
        ring: 'ring-emerald-500/10',
        title: '操作成功',
      };
    case 'error':
      return {
        icon: 'cross' as const,
        iconWrap: 'text-rose-700',
        ring: 'ring-rose-500/10',
        title: '发生错误',
      };
    case 'warning':
      return {
        icon: 'warning' as const,
        iconWrap: 'text-amber-700',
        ring: 'ring-amber-500/10',
        title: '提示警告',
      };
    case 'info':
    default:
      return {
        icon: 'info' as const,
        iconWrap: 'text-indigo-700',
        ring: 'ring-indigo-500/10',
        title: '系统通知',
      };
  }
});
</script>

<template>
  <!-- 遮罩层 (如果是二次确认弹窗模式则提供温和半透明背景毛玻璃聚焦) -->
  <Transition name="pony-overlay">
    <div
      v-if="activeItem && isConfirm"
      class="fixed inset-0 z-49 bg-slate-900/25 backdrop-blur-[2px] transition-opacity"
      data-testid="toast-overlay"
      @click="handleConfirmAction(false)"
    />
  </Transition>

  <!-- 居中毛玻璃 Toast/Confirm 容器 -->
  <Transition name="pony-toast-center">
    <div
      v-if="activeItem"
      class="fixed top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2 z-50 select-none pointer-events-auto"
      :class="[isConfirm ? 'w-[90vw] max-w-md' : 'max-w-lg min-w-[280px] w-auto']"
      data-testid="ui-toast"
      role="alert"
    >
      <div
        class="relative overflow-hidden rounded-2xl p-5 shadow-2xl backdrop-blur-2xl border transition-all"
        :class="[
          'bg-white/45 text-slate-900 border-white/40 shadow-slate-900/10 ring-1',
          typeConfig.ring,
        ]"
      >
        <!-- 内部顶层高透微渐变光感 (增强毛玻璃流光与通透度) -->
        <div class="absolute -top-10 -right-10 w-36 h-36 rounded-full bg-gradient-to-br from-white/30 to-transparent blur-lg pointer-events-none" />

        <div class="relative flex items-start gap-3.5">
          <!-- 语义图标 (无背景、无边框、颜色更深) -->
          <div
            class="w-7 h-7 flex items-center justify-center shrink-0 transition-transform duration-200"
            :class="typeConfig.iconWrap"
            data-testid="toast-icon-wrap"
          >
            <Icons :name="typeConfig.icon" size="22" />
          </div>

          <!-- 内容主体 -->
          <div class="flex-1 pt-0.5 min-w-0 pr-6">
            <h4
              v-if="activeItem.title || isConfirm"
              class="text-sm font-bold text-slate-900 tracking-tight mb-1 flex items-center gap-2"
              data-testid="toast-title"
            >
              {{ activeItem.title || typeConfig.title }}
            </h4>
            <div
              class="text-sm font-medium text-slate-700 leading-relaxed break-words"
              data-testid="toast-message"
            >
              {{ activeItem.message }}
            </div>
          </div>

          <!-- 手动关闭按钮 (仅非 Confirm 或显式 closable 时展示) -->
          <button
            v-if="activeItem.closable && !isConfirm"
            type="button"
            class="absolute top-0 right-0 p-1.5 rounded-lg text-slate-400 hover:text-slate-700 hover:bg-white/40 transition-colors cursor-pointer"
            aria-label="关闭通知"
            data-testid="toast-close-btn"
            @click="dismissToast(activeItem.id)"
          >
            <Icons name="cross" size="15" />
          </button>
        </div>

        <!-- 二次确认交互按钮组 -->
        <div
          v-if="isConfirm"
          class="mt-5 pt-3.5 border-t border-slate-900/10 flex items-center justify-end gap-2.5"
          data-testid="toast-confirm-actions"
        >
          <UiButton
            variant="ghost"
            size="sm"
            class="px-3.5 text-slate-700 hover:text-slate-900 hover:bg-white/40"
            data-testid="toast-cancel-btn"
            @click="handleConfirmAction(false)"
          >
            {{ activeItem.cancelText || '取消' }}
          </UiButton>
          <UiButton
            :variant="activeItem.variant || 'destructive'"
            size="sm"
            class="px-4 font-semibold shadow-sm"
            data-testid="toast-ok-btn"
            @click="handleConfirmAction(true)"
          >
            {{ activeItem.confirmText || '确定' }}
          </UiButton>
        </div>
      </div>
    </div>
  </Transition>
</template>

<style scoped>
/* 居中弹出与收缩动画：具有弹性与现代极简质感 */
.pony-toast-center-enter-active {
  transition: all 0.22s cubic-bezier(0.16, 1, 0.3, 1);
}
.pony-toast-center-leave-active {
  transition: all 0.18s cubic-bezier(0.4, 0, 1, 1);
}
.pony-toast-center-enter-from {
  opacity: 0;
  transform: translate(-50%, -46%) scale(0.94);
}
.pony-toast-center-leave-to {
  opacity: 0;
  transform: translate(-50%, -48%) scale(0.96);
}

.pony-overlay-enter-active,
.pony-overlay-leave-active {
  transition: opacity 0.2s ease;
}
.pony-overlay-enter-from,
.pony-overlay-leave-to {
  opacity: 0;
}
</style>
