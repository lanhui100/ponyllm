<script setup lang="ts">
import { useRouter, useRoute } from 'vue-router';
import { useSessionStore } from '../stores/session';
import Icons from './ui/Icons.vue';
import PonyLogo from './ui/PonyLogo.vue';
import UiButton from './ui/UiButton.vue';
import UiTooltip from './ui/UiTooltip.vue';

const router = useRouter();
const route = useRoute();
const session = useSessionStore();

function handleLogout() {
  session.logout();
  void router.push('/connect');
}
</script>

<template>
  <header class="h-15 bg-slate-900/5 backdrop-blur-md sticky top-0 z-40 px-4 sm:px-8 flex items-center justify-between border-b border-slate-900/5 transition-colors">
    <div class="flex items-center gap-8">
      <div class="flex items-center gap-2.5">
        <PonyLogo :size="34" />
        <span class="font-bold text-base tracking-tight text-slate-900">PonyLLM</span>
      </div>

      <nav class="flex items-center gap-2 text-[15px] font-medium">
        <router-link
          to="/dashboard"
          class="px-3.5 py-1.5 rounded-lg transition-colors text-slate-700 hover:text-slate-950 hover:bg-slate-900/5"
          :class="{ '!text-slate-950 !bg-slate-900/10 font-semibold shadow-2xs': route.path === '/dashboard' }"
        >
          Dashboard
        </router-link>
        <router-link
          to="/governance"
          class="px-3.5 py-1.5 rounded-lg transition-colors text-slate-700 hover:text-slate-950 hover:bg-slate-900/5"
          :class="{ '!text-slate-950 !bg-slate-900/10 font-semibold shadow-2xs': route.path === '/governance' }"
        >
          模型管理
        </router-link>
        <router-link
          to="/recorder"
          class="px-3.5 py-1.5 rounded-lg transition-colors text-slate-700 hover:text-slate-950 hover:bg-slate-900/5"
          :class="{ '!text-slate-950 !bg-slate-900/10 font-semibold shadow-2xs': route.path === '/recorder' }"
        >
          轨迹
        </router-link>
      </nav>
    </div>

    <div class="flex items-center gap-2">
      <UiTooltip content="退出网关连接">
        <UiButton
          variant="ghost"
          size="sm"
          class="text-slate-700 hover:text-rose-600 hover:bg-rose-500/10 text-[13px] gap-1.5 font-medium"
          @click="handleLogout"
        >
          <Icons name="lock" size="14" />
          <span>退出</span>
        </UiButton>
      </UiTooltip>
    </div>
  </header>
</template>
