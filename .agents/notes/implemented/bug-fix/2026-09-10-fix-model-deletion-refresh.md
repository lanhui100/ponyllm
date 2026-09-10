# Agent Note: Fix Model Deletion Refresh and Backend Scope

Status: implemented

## Problem
在 Web 控制台模型管理页面删除模型时，用户界面提示“删除成功”的 Toast，但页面上的模型条目并没有被移除，旧模型依然留在列表中。经排查发现存在两处根本原因：
1. 前端架构缺陷：Vue 3 的 `emit()` 函数并不返回父组件事件监听函数（如 `async removeModel`）的 Promise 或返回值。子组件 `ModelSubSection.vue` 中使用 `await emit('delete', name)` 得到的是 `undefined`，导致后面的 `toast.success` 在网络请求未完成、甚至发生 404/412 异常时就被当作成功触发，且异常无法在子组件内被捕获。同时，删除操作未传递所属 provider，导致跨提供商查找同名或默认模型时行为不精确。
2. 后端逻辑缺陷：后端的 `DELETE /api/admin/models/{name}` 仅从 `file.providers.get_mut(target_provider).models` 和 `model_configs` 中剔除该模型，但未检查并重置提供商上的 `default_model`。如果被删除的模型刚好是该提供商的 `default_model`，网关在 `list_all_models` 以及运行时仍然会根据 `default_model` 生成该模型的 `ModelSpec`。因此后续无论是前端刷新 `adminApi.getModels()` 还是查看网关模型，已删除的模型依然“死而复生”地出现在列表中。

## Decision
1. 后端支持与健全：
   - 在 `handle_admin_delete_model` 中，当被删除的模型等于该服务商的 `default_model` 时，将 `p_sec.default_model` 以及内存中对应服务商的 `default_model` 重置（优先降级为服务商剩余的第一个模型，若无剩余模型则置空），并同步将内存 `p_cfg.models` 与 `p_cfg.model_specs` 中该模型彻底移除。
2. 前端契约修复：
   - 在 `useAdminConfig.ts` 中，`removeModel(name: string, provider?: string)` 支持可选传递 `provider` 参数，并在请求 API 时携带 `?provider=...` 查询参数。
   - 在 `ModelSubSection.vue` 与 `ProviderCard.vue` 之间，显式将删除回调函数以 prop（`onDeleteModel?: (name: string, provider?: string) => Promise<void>`）注入（同时向后兼容触发 `emit('delete', name)`），使子组件可以直接 `await props.onDeleteModel(name, providerName)`。在删除失败时抛出错误进入 catch 分支提示失败，删除成功后再提示成功 Toast，并确保 `fetchAll()` 状态拉取彻底同步完成。

## Alternatives considered
- 方案 A（仅前端处理）：通过 Vue 3 自定义 Promise 包装传递给 emit。缺点：Vue 3 的 emit 机制设计为单向事件通知而非请求响应式 RPC，类型和运行时均不支持双向 Promise 返回；且如果后端依然保留 `default_model`，即便前端刷新，接口返回的模型列表依旧包含已删除模型，无法彻底解决问题。
- 方案 B（禁止删除 default_model）：当用户删除的是默认模型时后端直接报错拒绝。缺点：控制台用户体验极差，删除模型时可能并不知道该模型碰巧被填在了 default_model 字段，导致删除无法执行。自动将其迁移到其他剩余模型或置空更符合运维直觉。

## Consequences
- 彻底解决模型删除后“误报成功但依然留在列表中”的 Bug。
- 后端和前端保持对 `provider` 命名空间的支持，防止同名模型在不同提供商之间发生误删或查找失败。
- 前后端状态一致性得到保证，删除默认模型也能正确清理和刷新。
