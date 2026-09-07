<template>
  <main class="connect">
    <h1>连接网关</h1>
    <p v-if="openMode">网关为免鉴模式，无需输入 Token。</p>
    <form v-else @submit.prevent="submit">
      <label>
        Token
        <input v-model="input" type="password" autocomplete="off" placeholder="sk-pony-..." />
      </label>
      <button type="submit">连接</button>
      <p v-if="error" class="error">{{ error }}</p>
    </form>
  </main>
</template>

<script setup lang="ts">
import { ref, onMounted } from 'vue';
import { useRoute, useRouter } from 'vue-router';
import { useSessionStore } from '../stores/session';
import { bearerValue } from '../lib/alova';
import { PROBE_PATH, probeOpenMode } from '../router';

const input = ref('');
const error = ref('');
const openMode = ref(false);
const route = useRoute();
const router = useRouter();
const session = useSessionStore();

onMounted(async () => {
  openMode.value = await probeOpenMode().catch(() => false);
});

async function submit(): Promise<void> {
  error.value = '';
  const candidate = input.value.trim();
  if (candidate === '') {
    error.value = '请输入 Token';
    return;
  }
  try {
    const resp = await fetch(PROBE_PATH, {
      headers: { Authorization: bearerValue(candidate) },
    });
    // Tight: only 2xx logs in (P1-4). 401 => bad token; anything else (403/404/
    // 500) surfaces its status instead of silently "succeeding".
    if (resp.status === 401) {
      error.value = 'Token 无效（401）';
      return;
    }
    if (!resp.ok) {
      error.value = `网关异常（${resp.status}）`;
      return;
    }
    session.login(candidate);
    const redirect = route.query.redirect;
    await router.push(typeof redirect === 'string' ? redirect : '/dashboard');
  } catch {
    error.value = '网关不可达';
  }
}
</script>

<style scoped>
.connect {
  max-width: 420px;
  margin: 12vh auto;
  font-family: system-ui, sans-serif;
}
.error {
  color: #b3261e;
}
</style>
