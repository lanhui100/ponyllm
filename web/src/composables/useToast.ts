import { ref } from 'vue';

export type ToastType = 'success' | 'error' | 'warning' | 'info';

export interface ToastOptions {
  id?: string;
  type?: ToastType;
  title?: string;
  message: string;
  duration?: number; // 默认 3000ms，0 代表不自动关闭
  closable?: boolean;
}

export interface ConfirmOptions {
  title?: string;
  message: string;
  type?: ToastType;
  confirmText?: string;
  cancelText?: string;
  variant?: 'default' | 'destructive';
}

export interface ToastItem extends ToastOptions {
  id: string;
  type: ToastType;
  timer?: ReturnType<typeof setTimeout> | null;
  isConfirm?: boolean;
  confirmText?: string;
  cancelText?: string;
  variant?: 'default' | 'destructive';
  resolve?: (value: boolean) => void;
}

// 模块级单例状态
const currentToast = ref<ToastItem | null>(null);

function clearCurrentTimer() {
  if (currentToast.value?.timer) {
    clearTimeout(currentToast.value.timer);
    currentToast.value.timer = null;
  }
}

export function dismissToast(id?: string) {
  if (!currentToast.value) return;
  if (!id || currentToast.value.id === id) {
    clearCurrentTimer();
    if (currentToast.value.resolve) {
      currentToast.value.resolve(false);
    }
    currentToast.value = null;
  }
}

export function showToastNotification(options: ToastOptions | string, type: ToastType = 'info'): string {
  clearCurrentTimer();
  const id = `toast-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`;
  const normalized: ToastOptions =
    typeof options === 'string'
      ? { message: options, type, duration: 3000 }
      : { duration: 3000, type, ...options };

  const item: ToastItem = {
    ...normalized,
    id: normalized.id || id,
    type: normalized.type || type,
    closable: normalized.closable ?? true,
  };

  const duration = item.duration ?? 3000;
  if (duration > 0) {
    item.timer = setTimeout(() => {
      dismissToast(item.id);
    }, duration);
  }

  currentToast.value = item;
  return item.id;
}

export function showConfirmDialog(options: ConfirmOptions | string): Promise<boolean> {
  clearCurrentTimer();
  const id = `confirm-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`;
  const normalized: ConfirmOptions =
    typeof options === 'string'
      ? { message: options, title: '请确认', type: 'warning' }
      : { title: '请确认', type: 'warning', ...options };

  return new Promise<boolean>((resolve) => {
    currentToast.value = {
      id,
      title: normalized.title,
      message: normalized.message,
      type: normalized.type || 'warning',
      duration: 0,
      closable: true,
      isConfirm: true,
      confirmText: normalized.confirmText || '确定',
      cancelText: normalized.cancelText || '取消',
      variant: normalized.variant || 'destructive',
      resolve: (val: boolean) => {
        resolve(val);
      },
    };
  });
}

export function handleConfirmAction(confirmed: boolean) {
  if (currentToast.value?.resolve) {
    currentToast.value.resolve(confirmed);
  }
  clearCurrentTimer();
  currentToast.value = null;
}

export const toast = {
  show: showToastNotification,
  success: (message: string, options?: Omit<ToastOptions, 'message' | 'type'>) =>
    showToastNotification({ ...options, message, type: 'success' }),
  error: (message: string, options?: Omit<ToastOptions, 'message' | 'type'>) =>
    showToastNotification({ ...options, message, type: 'error' }),
  warning: (message: string, options?: Omit<ToastOptions, 'message' | 'type'>) =>
    showToastNotification({ ...options, message, type: 'warning' }),
  info: (message: string, options?: Omit<ToastOptions, 'message' | 'type'>) =>
    showToastNotification({ ...options, message, type: 'info' }),
  confirm: showConfirmDialog,
  dismiss: dismissToast,
};

export function useToast() {
  return {
    toast,
    currentToast,
    dismissToast,
    handleConfirmAction,
  };
}
