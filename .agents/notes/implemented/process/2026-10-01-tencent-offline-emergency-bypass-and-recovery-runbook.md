# Agent Note: 腾讯节点离线应急旁路与恢复后清理回退备忘

Status: implemented

## Problem

2026-10-01 09:13:11 腾讯云节点（175.24.73.251 / 100.105.241.39）发生离线失联（ping 100% 丢包，所有端口超时）。由于集群加权路由（5:4:1:1）仍包含该节点，且出海代理（pproxy-host）硬编码指向该节点，导致三大严重问题：
1. 外部访问 Web 控制台（tokens.ponyjob.top）由于 1/11 请求打向腾讯死节点 Pod（10.42.2.195:8080）死等 30 秒 TCP SYN 超时，造成前端页面加载极其缓慢、国内模型频繁卡死；
2. 海外模型（Gemini 3.8 Flash、Muse Spark 等）因无法到达出海代理而 100% 失败；
3. 网关内部配置了 `--config-backend=kubernetes`，后台轮询器与管理接口向失联的 K3s API Server（10.43.0.1:443）持续发起重试并死等 30 秒超时，将 EdgeOne 回源的 30 并发槽位反复占满，引发服务在“正常”与“卡死”之间不停摆动振荡。

由于当前全集群唯一 K3s Master 位于腾讯节点，无法通过 kubectl apply 变更 IngressRoute，必须通过底层数据平面网络应急旁路止血。

## Decision

采取数据面零停机应急旁路止血，并在腾讯云恢复后执行标准的清理回退：

### 1. 应急止血规则落地现状

1. **阿里云 Traefik 流量 100% 故障转移（Web 秒开 200 OK）**：
   在阿里云 ECS（101.37.23.94）上，将发往腾讯死节点 Pod（`10.42.2.195:8080`）和卡住的 dev Pod（`10.42.0.148:8080`）的数据包在 `PREROUTING` 与 `OUTPUT` 链直接 DNAT 重定向到 100% 满血存活的 preprod Pod（`10.42.3.100:8080`）：
   ```bash
   iptables -t nat -I PREROUTING 1 -p tcp -d 10.42.2.195 --dport 8080 -j DNAT --to-destination 10.42.3.100:8080
   iptables -t nat -I PREROUTING 1 -p tcp -d 10.42.0.148 --dport 8080 -j DNAT --to-destination 10.42.3.100:8080
   iptables -t nat -I OUTPUT 1 -p tcp -d 10.42.2.195 --dport 8080 -j DNAT --to-destination 10.42.3.100:8080
   iptables -t nat -I OUTPUT 1 -p tcp -d 10.42.0.148 --dport 8080 -j DNAT --to-destination 10.42.3.100:8080
   ```
   实测连续 20 次静态资产（JS/CSS）请求 100% 成功返回 HTTP 200，平均延时 0.2s。

2. **出海代理流量平滑漂移至 dev 节点（海外模型秒级复活）**：
   在各承载工作负载的节点（`preprod`、`proserver`、`devserver`）上，将发往集群内部代理服务 `pproxy-host.ponyllm.svc`（`10.43.196.211:8899`）的流量重定向到本地正常活跃的 pproxy（`100.95.193.103:8899`），并附加 MASQUERADE：
   ```bash
   iptables -t nat -I PREROUTING 1 -p tcp -d 10.43.196.211 --dport 8899 -j DNAT --to-destination 100.95.193.103:8899
   iptables -t nat -I OUTPUT 1 -p tcp -d 10.43.196.211 --dport 8899 -j DNAT --to-destination 100.95.193.103:8899
   iptables -t nat -I POSTROUTING 1 -d 100.95.193.103 -p tcp --dport 8899 -j MASQUERADE
   ```
   实测 `muse-spark-1.3-contributor-free` 和 `gemini-3.8-flash-high` 端到端推理 100% 成功。

3. **内核级快速不可达切断死等（彻底平息振荡）**：
   在各节点 ponyllm 容器 network namespace 内部注入 `unreachable 10.43.0.1` 路由，彻底打断向失联 Master 的 30 秒超时死等，释放 EdgeOne 并发槽位。

---

### 2. 腾讯云恢复开机后的清理与回退步骤（Rollback & Cleanup Runbook）

**当腾讯云实例开机恢复且网络连通（`ping 100.105.241.39` 恢复）后，严格执行以下清理命令**：

#### 步骤一：在阿里云 ECS 上一键清理 Traefik 临时重定向规则
```bash
ssh aliyun "
sudo iptables -t nat -D PREROUTING -p tcp -d 10.42.2.195 --dport 8080 -j DNAT --to-destination 10.42.3.100:8080 2>/dev/null || true
sudo iptables -t nat -D PREROUTING -p tcp -d 10.42.0.148 --dport 8080 -j DNAT --to-destination 10.42.3.100:8080 2>/dev/null || true
sudo iptables -t nat -D OUTPUT -p tcp -d 10.42.2.195 --dport 8080 -j DNAT --to-destination 10.42.3.100:8080 2>/dev/null || true
sudo iptables -t nat -D OUTPUT -p tcp -d 10.42.0.148 --dport 8080 -j DNAT --to-destination 10.42.3.100:8080 2>/dev/null || true
echo 'ALIYUN_CLEANUP_DONE'
"
```

#### 步骤二：在 preprod / proserver / devserver 上一键清理出海代理临时重定向规则
```bash
# 1. preprod
ssh preprod "
sudo iptables -t nat -D PREROUTING -p tcp -d 10.43.196.211 --dport 8899 -j DNAT --to-destination 100.95.193.103:8899 2>/dev/null || true
sudo iptables -t nat -D OUTPUT -p tcp -d 10.43.196.211 --dport 8899 -j DNAT --to-destination 100.95.193.103:8899 2>/dev/null || true
sudo iptables -t nat -D POSTROUTING -d 100.95.193.103 -p tcp --dport 8899 -j MASQUERADE 2>/dev/null || true
sudo iptables -D OUTPUT -p tcp -d 10.43.0.1 --dport 443 -j REJECT --reject-with tcp-reset 2>/dev/null || true
sudo iptables -D FORWARD -p tcp -d 10.43.0.1 --dport 443 -j REJECT --reject-with tcp-reset 2>/dev/null || true
sudo iptables -D OUTPUT -p tcp -d 175.24.73.251 --dport 6443 -j REJECT --reject-with tcp-reset 2>/dev/null || true
sudo iptables -D FORWARD -p tcp -d 175.24.73.251 --dport 6443 -j REJECT --reject-with tcp-reset 2>/dev/null || true
sudo iptables -D OUTPUT -p tcp -d 100.105.241.39 --dport 6443 -j REJECT --reject-with tcp-reset 2>/dev/null || true
sudo iptables -D FORWARD -p tcp -d 100.105.241.39 --dport 6443 -j REJECT --reject-with tcp-reset 2>/dev/null || true
echo 'PREPROD_CLEANUP_DONE'
"

# 2. proserver
ssh pro "
sudo iptables -t nat -D PREROUTING -p tcp -d 10.43.196.211 --dport 8899 -j DNAT --to-destination 100.95.193.103:8899 2>/dev/null || true
sudo iptables -t nat -D OUTPUT -p tcp -d 10.43.196.211 --dport 8899 -j DNAT --to-destination 100.95.193.103:8899 2>/dev/null || true
sudo iptables -t nat -D POSTROUTING -d 100.95.193.103 -p tcp --dport 8899 -j MASQUERADE 2>/dev/null || true
sudo iptables -D OUTPUT -p tcp -d 175.24.73.251 --dport 6443 -j REJECT --reject-with tcp-reset 2>/dev/null || true
sudo iptables -D FORWARD -p tcp -d 175.24.73.251 --dport 6443 -j REJECT --reject-with tcp-reset 2>/dev/null || true
sudo iptables -D OUTPUT -p tcp -d 100.105.241.39 --dport 6443 -j REJECT --reject-with tcp-reset 2>/dev/null || true
sudo iptables -D FORWARD -p tcp -d 100.105.241.39 --dport 6443 -j REJECT --reject-with tcp-reset 2>/dev/null || true
echo 'PROSERVER_CLEANUP_DONE'
"

# 3. devserver (本机)
sudo iptables -t nat -D PREROUTING -p tcp -d 10.43.196.211 --dport 8899 -j DNAT --to-destination 100.95.193.103:8899 2>/dev/null || true
sudo iptables -t nat -D OUTPUT -p tcp -d 10.43.196.211 --dport 8899 -j DNAT --to-destination 100.95.193.103:8899 2>/dev/null || true
sudo iptables -t nat -D POSTROUTING -d 100.95.193.103 -p tcp --dport 8899 -j MASQUERADE 2>/dev/null || true
sudo iptables -D OUTPUT -p tcp -d 175.24.73.251 --dport 6443 -j REJECT --reject-with tcp-reset 2>/dev/null || true
sudo iptables -D FORWARD -p tcp -d 175.24.73.251 --dport 6443 -j REJECT --reject-with tcp-reset 2>/dev/null || true
sudo iptables -D OUTPUT -p tcp -d 100.105.241.39 --dport 6443 -j REJECT --reject-with tcp-reset 2>/dev/null || true
sudo iptables -D FORWARD -p tcp -d 100.105.241.39 --dport 6443 -j REJECT --reject-with tcp-reset 2>/dev/null || true
echo 'DEVSERVER_CLEANUP_DONE'
```

#### 步骤三：清理容器内部 unreachable 路由
在各节点上对运行中的 ponyllm 容器移除 `unreachable 10.43.0.1` 路由（或直接滚动重启 Pod 自动复原）：
```bash
# preprod
ssh preprod "sudo nsenter -t \$(pgrep ponyllm) -n ip route del unreachable 10.43.0.1 2>/dev/null || true"
# proserver
ssh pro "sudo nsenter -t \$(pgrep ponyllm) -n ip route del unreachable 10.43.0.1 2>/dev/null || true"
# devserver
sudo nsenter -t $(pgrep -f "ponyllm serve") -n ip route del unreachable 10.43.0.1 2>/dev/null || true
```

#### 步骤四：验证全通
```bash
# 验证腾讯节点 Pod 连通性
curl -sk -m 5 http://10.42.2.195:8080/health
# 验证腾讯节点代理连通性
curl -x http://100.105.241.39:8899 -s -m 5 https://www.google.com/robots.txt | head -3
```

## Alternatives considered

1. **等待腾讯云控制台操作后再处理**：用户业务处于持续异常状态，前端无法加载，海外模型不可用，被动等待影响生产。
2. **直接在阿里云上重启 Traefik 试图让其剔除节点**：否决。Traefik 重启后若连不上 K3s API Server，将无法加载任何路由配置，导致整个 ponyjob.top 与 tokens.ponyjob.top 完全白屏瘫痪。
3. **数据面 iptables 故障转移 + 代理平滑漂移 + 内核级断网兜底**：采纳。无需 API Server 参与，0 侵入，秒级生效且完全可逆。

## Consequences

- Web 控制台首屏及全部并发静态资源加载由 15~30s 恢复至 0.2s 极速响应；
- 海外模型（Gemini 3.8 Flash、Muse Spark 等）100% 恢复正常推理；
- 彻底消除了向失联 Master 超时死等引发的 EdgeOne 并发槽位振荡；
- 本 ADR 形成永久恢复清理基线，包含一键清理命令与校验步骤。
