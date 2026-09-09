# Agent Note: upgrade下载链路韧性修复

Status: implemented

## Problem
`ponyllm upgrade` 在 dev 服务器上必失败：主源经代理到 GitHub release-assets 实测约 20KB/s，6.48MB 需 5 分钟以上，而下载复用 30s 超时的 API client，必超时；两个备选镜像（ghfast.top、ghproxy.net）被出口代理 CONNECT 403，全灭；且最终报错只保留最后一次尝试的错误，把真正的主源超时信息吞掉，误导排查。

## Decision
下载与 API 查询分离：API client 保持 30s 超时，新增下载专用 client 超时 600s；下载改分块流式写入内存并按 MB 打印进度（慢链路可见、不再“假死”）；保留现有镜像 fallback 顺序（主源优先，其它网络下仍有效）；聚合每次尝试的 URL 与错误并全部输出，单次失败不再掩盖主因。

## Alternatives considered
- 换镜像源：实测 6 个常用镜像在本代理下全部 CONNECT 403，换源无用，否决。
- 仅调大全局超时：API 查询不需要 600s，大超时掩盖 API 侧真正故障，否决；故双 client 分离。
- 流式落盘到文件：6MB 级资产内存缓冲足够，落盘增加临时文件生命周期管理复杂度，否决；保持返回 `Vec<u8>`，extract 流程零改动。
- 去掉镜像 fallback：其它用户网络下镜像仍可能有效，保留零成本（被墙时失败极快），否决。

## Consequences
- 慢链路下升级耗时数分钟但可成功，进度可见；`--force` 可端到端验证。
- 超时常量 `DOWNLOAD_TIMEOUT_SECS=600` 写死，若资产体积大一个数量级需重调（靠 review）。
