# Agent Note: 修复 Provider 模型协议编辑态被后台轮询自动清空与取消的问题

Status: implemented

## Problem

在 Web 界面的模型管理（`GovernanceView`）中，用户点击服务商卡片的「配置端点」进入协议编辑状态后：
当勾选 `Anthropic Messages` 或 `OpenAI Responses` 协议，并在对应的专属 Base URL 输入框中进行输入时，如果输入过程持续数秒（或输入完成尚未点击保存），输入框内容会被自动清空，且刚刚选中的协议药丸胶囊会被自动取消勾选，导致输入框消失。

排查发现其根因为：
1. `GovernanceView.vue` 中挂载了 5 秒周期的轻量定时器 `syncTimer`，每 5 秒自动触发 `refreshSilent()` 静默向后端同步最新的提供商列表数据 `providers`。
2. `ProviderCard.vue` 中设置了 `watch(() => props.provider, (p) => { ... }, { deep: true })`，负责将父组件更新的 `provider` 属性重置到子组件的内部状态 `activeProtocols` 与 `customUrls`。
3. 该 `watch` 监听器未做“当前是否正处于编辑中”的防卫判断。当后台静默同步拉取到远端尚未持久化的旧数据时，直接用旧的 `provider` 覆盖了本地未提交的编辑状态，导致用户的输入被直接回滚。

## Decision

在 `web/src/components/governance/ProviderCard.vue` 的 `watch(() => props.provider)` 监听器中增加编辑状态防卫：

```ts
watch(
  () => props.provider,
  (p) => {
    // 防卫：编辑状态中拒绝后台静默轮询的属性重置，避免打断用户输入
    if (isEditingProtocols.value) {
      return;
    }
    activeProtocols.value = getInitialProtocols();
    customUrls.value = {
      chat: p.chat_url || '',
      messages: p.messages_url || '',
      responses: p.responses_url || '',
    };
  },
  { deep: true }
);
```

当且仅当用户处于非编辑状态（展示态）时，接受来自父层轮询的 `props.provider` 更新；
当处于编辑态（`isEditingProtocols.value === true`）时，阻断外部刷新对内部草稿状态的覆盖。
若用户主动点击「取消」按钮，现有的 `cancelEditProtocols()` 函数已具备显式根据最新 `props.provider` 重置草稿并退出编辑态的能力；若用户点击「保存」，在保存成功并退出编辑态后，下一次轮询或主动刷新即可无缝同步远端保存结果。

## Alternatives considered

1. **在 `GovernanceView` 的 `canAutoSync()` 中全局感知每个 Provider 的编辑态并暂停轮询**：
   - 缺点：需要自下而上向父组件冒泡 `editing-state-change` 事件，使得状态跨层级耦合，增加维护负担；且一个卡片在编辑会导致整个治理页面的连接池状态无法静默同步。
2. **完全移除 `ProviderCard.vue` 的 `watch(() => props.provider)`**：
   - 缺点：当外部真正发生刷新（例如管理员在其它窗口修改了端点、或者手动点击刷新按钮）时，展示态的协议与端点无法自动响应更新。
3. **在 `watch` 内做局部防卫（采纳）**：
   - 仅阻断编辑态时的静默覆盖，边界清晰，内聚在 `ProviderCard` 内部，不产生跨组件的额外状态暴露。

## Consequences

- 用户在修改模型协议药丸胶囊、输入 Anthropic / Responses 专属 Base URL 时，不再会被 5s 一次的后台静默轮询打断或清空。
- `ProviderCard.test.ts` 补充该防卫逻辑的单元测试，确保回归可测。
