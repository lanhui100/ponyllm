# 流水线发布门禁加固与RBAC回滚权限补全

Status: implemented

- 日期：2026-10-11

## Context

在近期生产发布中暴露了两个工程隐患：
1. **参数传递前置静态契约缺失**：发布脚本 `serialized-rollout.sh` 与探针 `prober.py` 的参数传递错误，未能在代码测试与静态检查阶段被发现，直到跨平台测试及镜像构建完成（近 15 分钟）后的最后滚动发布阶段才暴露，延长了反馈链路；
2. **回滚权限不足**：当探针检查失败触发逆序级联回滚 `kubectl rollout undo` 时，报错 `replicasets.apps is forbidden: User system:serviceaccount:ponyllm:ci-deployer cannot list resource replicasets`，导致回滚受阻；
3. **部署后前端版本更新缺乏即时端到端校验**：缺少在线静态资产指纹与最新构建一致性的自动检测。

## Decision

1. **RBAC 最小权限补全**：在 `deploy/ci-deployer.yaml` 中为 `ci-deployer` 的 `apps` 资源规则中添加 `replicasets` 权限，确保 `rollout undo` 能顺利查找并切回历史 ReplicaSet。
2. **前置参数契约硬门禁**：新增 `tests/pre-flight/check-ci-prober-cli-contract.sh` 并在 CI 首个测试阶段运行，验证探针参数解析能力、参数透传及 dry-run 完整链路。
3. **部署后前端版本一致性探针**：在 `scripts/post-deploy-smoke.sh` 中增加第 2 步 SPA 资产一致性校验，比对本地构建产物 `index.html` 引用的 JS bundle 与线上实际下发的 HTML 资产，不一致即非零退出，阻断 CDN 缓存滞后或假发布。

## Alternatives considered

- **仅依赖人工肉眼巡检线上版本**：无法作为自动化流水线阻断器，容易引发版本裂脑遗留。
- **给 ci-deployer 授予 cluster-admin 或更宽泛命名空间权限**：违背最小权限原则。仅开放当前命名空间的 `replicasets` 是最精准解法。

## Consequences

- 逆序级联回滚具备了完整的底层 Kubernetes 权限支撑；
- 部署参数契约问题可在本地提交前及 CI 首步秒级发现，大幅降低迭代与排障成本；
- 生产环境发布具备端到端前后端双重物理验收收据。
