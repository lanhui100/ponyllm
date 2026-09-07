<script setup lang="ts">
import { ref } from 'vue';
import { useAdminConfig } from '../composables/useAdminConfig';
import NavBar from '../components/NavBar.vue';
import ProviderSection from '../components/governance/ProviderSection.vue';
import ModelSection from '../components/governance/ModelSection.vue';
import KeySection from '../components/governance/KeySection.vue';
import StrategySection from '../components/governance/StrategySection.vue';
import KeySecretModal from '../components/governance/KeySecretModal.vue';
import ConflictModal from '../components/governance/ConflictModal.vue';

const {
  overview,
  providers,
  models,
  keys,
  strategy,
  configVersion,
  loading,
  error,
  adminWriteEnabled,
  conflictDetected,
  createdKeyResult,
  keyTestResults,
  testingKeyIds,
  batchTesting,
  fetchAll,
  saveProvider,
  removeProvider,
  saveModel,
  editModel,
  removeModel,
  addKey,
  removeKey,
  testSingleKey,
  batchTestAllKeys,
  saveStrategy,
  clearConflict,
  clearCreatedKeyResult,
} = useAdminConfig({ autoFetch: true });

type TabType = 'providers' | 'models' | 'keys' | 'strategy';
const currentTab = ref<TabType>('providers');

async function handleRefresh() {
  clearConflict();
  await fetchAll().catch(() => {});
}

async function handleCreateProvider(payload: any) {
  try {
    await saveProvider(payload);
  } catch {
    // Conflict is surfaced via conflictDetected
  }
}

async function handleCreateModel(payload: any) {
  try {
    await saveModel(payload);
  } catch {
    // Conflict is surfaced via conflictDetected
  }
}

async function handleUpdateModel(name: string, payload: any) {
  try {
    await editModel(name, payload);
  } catch {
    // Conflict is surfaced via conflictDetected
  }
}

async function handleCreateKey(payload: any) {
  try {
    await addKey(payload);
  } catch {
    // Conflict is surfaced via conflictDetected
  }
}
</script>

<template>
  <div class="governance-page">
    <NavBar />

    <main class="page-content">
      <!-- 只读模式灰度提示条 -->
      <div
        v-if="!adminWriteEnabled"
        class="readonly-banner"
        data-testid="readonly-banner"
      >
        <div class="banner-icon">🔒</div>
        <div class="banner-text">
          <strong>只读治理模式：</strong>
          当前网关服务端未启用写权限（<code>admin_write_enabled=false</code>）。所有新增、修改、删除与敏感探针操作已被锁定。如需在网页控制台管理配置，请在服务端配置开启该项。
        </div>
      </div>

      <!-- 顶部标题与概要信息 -->
      <div class="header-row">
        <div>
          <h1 class="page-title">模型管理中心</h1>
          <p class="page-desc">动态管理模型、服务商、密钥池与全局分流策略</p>
        </div>

        <div class="header-stats">
          <div class="stat-pill">
            <span class="stat-label">配置版本:</span>
            <span class="stat-value" data-testid="config-version-val">v{{ configVersion }}</span>
          </div>
          <button
            type="button"
            class="refresh-btn"
            :disabled="loading"
            data-testid="refresh-all-btn"
            @click="handleRefresh"
          >
            {{ loading ? '刷新中...' : '刷新数据' }}
          </button>
        </div>
      </div>

      <!-- 错误提示 -->
      <div v-if="error" class="error-banner">
        <span>获取配置失败: {{ error }}</span>
        <button type="button" class="retry-btn" @click="handleRefresh">重试</button>
      </div>

      <!-- 选项卡导航 -->
      <div class="tabs-nav">
        <button
          type="button"
          class="tab-btn"
          :class="{ active: currentTab === 'providers' }"
          data-testid="tab-providers"
          @click="currentTab = 'providers'"
        >
          Providers 服务商 ({{ providers.length }})
        </button>
        <button
          type="button"
          class="tab-btn"
          :class="{ active: currentTab === 'models' }"
          data-testid="tab-models"
          @click="currentTab = 'models'"
        >
          Models 模型字典 ({{ models.length }})
        </button>
        <button
          type="button"
          class="tab-btn"
          :class="{ active: currentTab === 'keys' }"
          data-testid="tab-keys"
          @click="currentTab = 'keys'"
        >
          Keys 密钥与拨测 ({{ keys.length }})
        </button>
        <button
          type="button"
          class="tab-btn"
          :class="{ active: currentTab === 'strategy' }"
          data-testid="tab-strategy"
          @click="currentTab = 'strategy'"
        >
          全局调度策略
        </button>
      </div>

      <!-- Tab 面板内容 -->
      <div class="tab-content">
        <ProviderSection
          v-if="currentTab === 'providers'"
          :providers="providers"
          :admin-write-enabled="adminWriteEnabled"
          @create="handleCreateProvider"
          @delete="removeProvider"
        />

        <ModelSection
          v-else-if="currentTab === 'models'"
          :models="models"
          :providers="providers"
          :admin-write-enabled="adminWriteEnabled"
          @create="handleCreateModel"
          @update="handleUpdateModel"
          @delete="removeModel"
        />

        <KeySection
          v-else-if="currentTab === 'keys'"
          :keys="keys"
          :providers="providers"
          :admin-write-enabled="adminWriteEnabled"
          :key-test-results="keyTestResults"
          :testing-key-ids="testingKeyIds"
          :batch-testing="batchTesting"
          @create="handleCreateKey"
          @delete="removeKey"
          @test-single="testSingleKey"
          @test-batch="batchTestAllKeys"
        />

        <StrategySection
          v-else-if="currentTab === 'strategy'"
          :current-strategy="strategy"
          :admin-write-enabled="adminWriteEnabled"
          @update="saveStrategy"
        />
      </div>
    </main>

    <!-- 一次性明文密钥展示弹窗 -->
    <KeySecretModal
      :key-result="createdKeyResult"
      @close="clearCreatedKeyResult"
    />

    <!-- 412 并发冲突提示与重载引导 -->
    <ConflictModal
      :show="conflictDetected"
      @refresh="handleRefresh"
      @close="clearConflict"
    />
  </div>
</template>

<style scoped>
.governance-page {
  min-height: 100vh;
  background: #f8fafc;
  font-family: system-ui, -apple-system, sans-serif;
}

.page-content {
  max-width: 1280px;
  margin: 0 auto;
  padding: 24px;
}

.readonly-banner {
  display: flex;
  align-items: center;
  gap: 12px;
  background: #fffbeb;
  border: 1px solid #fef3c7;
  padding: 12px 16px;
  border-radius: 8px;
  margin-bottom: 20px;
  color: #92400e;
  font-size: 13px;
  line-height: 1.5;
}

.banner-icon {
  font-size: 18px;
}

.readonly-banner code {
  background: #fde68a;
  padding: 1px 4px;
  border-radius: 4px;
  font-family: monospace;
}

.header-row {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 20px;
}

.page-title {
  font-size: 20px;
  font-weight: 700;
  color: #0f172a;
  margin: 0 0 4px 0;
}

.page-desc {
  font-size: 13px;
  color: #64748b;
  margin: 0;
}

.header-stats {
  display: flex;
  align-items: center;
  gap: 12px;
}

.stat-pill {
  padding: 6px 12px;
  background: #ffffff;
  border: 1px solid #e2e8f0;
  border-radius: 6px;
  font-size: 13px;
}

.stat-label {
  color: #64748b;
  margin-right: 6px;
}

.stat-value {
  font-weight: 700;
  color: #0f172a;
  font-family: monospace;
}

.refresh-btn {
  padding: 6px 14px;
  font-size: 13px;
  font-weight: 500;
  background: #ffffff;
  border: 1px solid #cbd5e1;
  border-radius: 6px;
  color: #334155;
  cursor: pointer;
  transition: all 0.15s;
}

.refresh-btn:hover {
  background: #f1f5f9;
}

.error-banner {
  display: flex;
  justify-content: space-between;
  align-items: center;
  background: #fef2f2;
  border: 1px solid #fee2e2;
  padding: 10px 14px;
  border-radius: 6px;
  color: #b91c1c;
  font-size: 13px;
  margin-bottom: 16px;
}

.retry-btn {
  background: transparent;
  border: 1px solid #fca5a5;
  color: #b91c1c;
  border-radius: 4px;
  padding: 2px 8px;
  cursor: pointer;
  font-size: 12px;
}

.tabs-nav {
  display: flex;
  gap: 8px;
  margin-bottom: 16px;
  border-bottom: 1px solid #e2e8f0;
  padding-bottom: 8px;
}

.tab-btn {
  padding: 8px 16px;
  font-size: 14px;
  font-weight: 600;
  color: #64748b;
  background: transparent;
  border: none;
  border-radius: 6px;
  cursor: pointer;
  transition: all 0.15s;
}

.tab-btn:hover {
  color: #0f172a;
  background: #f1f5f9;
}

.tab-btn.active {
  color: #2563eb;
  background: #eff6ff;
}

.tab-content {
  margin-top: 12px;
}
</style>
