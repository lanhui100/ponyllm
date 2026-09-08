<script setup lang="ts">
import { useRouter, useRoute } from 'vue-router';
import { useSessionStore } from '../stores/session';
import Icons from './ui/Icons.vue';
import UiButton from './ui/UiButton.vue';
import UiTooltip from './ui/UiTooltip.vue';

const router = useRouter();
const route = useRoute();
const session = useSessionStore();

function handleLogout() {
  session.clearToken();
  void router.push('/connect');
}
</script>

<template>
  <header class="h-14 bg-white/90 backdrop-blur-md sticky top-0 z-40 px-4 sm:px-8 flex items-center justify-between border-b border-slate-200/60 shadow-2xs">
    <div class="flex items-center gap-8">
      <div class="flex items-center gap-2">
        <div class="w-7 h-7 rounded-lg bg-blue-600 text-white flex items-center justify-center font-bold text-xs shadow-2xs">
          P
        </div>
        <span class="font-bold text-sm tracking-tight text-slate-900">PonyLLM</span>
      </div>

      <nav class="flex items-center gap-1 text-xs font-medium">
        <router-link
          to="/dashboard"
          class="px-3 py-1.5 rounded-lg transition-colors text-slate-600 hover:text-slate-900 hover:bg-slate-100/70"
          :class="{ '!text-blue-600 !bg-blue-50/80 font-semibold': route.path === '/dashboard' }"
        >
          Dashboard
        </router-link>
        <router-link
          to="/recorder"
          class="px-3 py-1.5 rounded-lg transition-colors text-slate-600 hover:text-slate-900 hover:bg-slate-100/70"
          :class="{ '!text-blue-600 !bg-blue-50/80 font-semibold': route.path === '/recorder' }"
        >
          可观测性
        </router-link>
        <router-link
          to="/governance"
          class="px-3 py-1.5 rounded-lg transition-colors text-slate-600 hover:text-slate-900 hover:bg-slate-100/70"
          :class="{ '!text-blue-600 !bg-blue-50/80 font-semibold': route.path === '/governance' }"
        >
          模型管理
        </router-link>
      </nav>
    </div>

    <div class="flex items-center gap-2">
      <UiTooltip content="退出网关连接">
        <UiButton
          variant="ghost"
          size="sm"
          class="text-slate-500 hover:text-rose-600 hover:bg-rose-50/80 text-xs gap-1.5 font-normal"
          @click="handleLogout"
        >
          <Icons name="lock" size="13" />
          <span>退出</span>
        </UiButton>
      </UiTooltip>
    </div>
  </header>
</template>
