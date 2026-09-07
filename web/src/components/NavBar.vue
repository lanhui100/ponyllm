<script setup lang="ts">
import { useRouter, useRoute } from 'vue-router';
import { useSessionStore } from '../stores/session';

const router = useRouter();
const route = useRoute();
const session = useSessionStore();

function handleLogout() {
  session.clearToken();
  void router.push('/connect');
}
</script>

<template>
  <header class="top-nav">
    <div class="nav-left">
      <span class="brand">PonyLLM Console</span>
      <nav class="nav-links">
        <router-link to="/dashboard" :class="{ active: route.path === '/dashboard' }">
          可观测大盘
        </router-link>
        <router-link to="/recorder" :class="{ active: route.path === '/recorder' }">
          黑匣子录波
        </router-link>
      </nav>
    </div>

    <div class="nav-right">
      <button class="logout-btn" @click="handleLogout">
        退出连接
      </button>
    </div>
  </header>
</template>

<style scoped>
.top-nav {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 0 24px;
  height: 56px;
  background: #ffffff;
  border-bottom: 1px solid #e2e8f0;
}

.nav-left {
  display: flex;
  align-items: center;
  gap: 32px;
}

.brand {
  font-weight: 700;
  font-size: 16px;
  color: #0f172a;
  letter-spacing: -0.02em;
}

.nav-links {
  display: flex;
  align-items: center;
  gap: 16px;
}

.nav-links a {
  text-decoration: none;
  font-size: 14px;
  font-weight: 500;
  color: #64748b;
  padding: 6px 12px;
  border-radius: 6px;
  transition: all 0.15s ease;
}

.nav-links a:hover {
  color: #0f172a;
  background: #f1f5f9;
}

.nav-links a.active {
  color: #2563eb;
  background: #eff6ff;
}

.logout-btn {
  padding: 6px 12px;
  font-size: 13px;
  color: #64748b;
  background: transparent;
  border: 1px solid #cbd5e1;
  border-radius: 6px;
  cursor: pointer;
  transition: all 0.15s ease;
}

.logout-btn:hover {
  color: #0f172a;
  border-color: #94a3b8;
  background: #f8fafc;
}
</style>
