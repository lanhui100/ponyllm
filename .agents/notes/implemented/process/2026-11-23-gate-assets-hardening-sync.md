# Agent Note: 门禁资产升级——A6 缺失 fail-closed、A8 检索面同口径、A5 补 `ponygo status`

Status: implemented

## Problem

治理门禁（`.meta/gates`）这三处原本可以被静默绕过，或同一个工作树得出两种结论：

1. **A6（归档封印）可被"删文件"摘掉**：旧 A1 `adr-verify.sh` 在找不到 `archive-verify.sh`
   时只打印一行跳过并放行。也就是说，删掉一个文件就能让归档封印门消失，而 pre-commit
   仍然绿灯——这是 fail-open，且没有任何日志会把它读成"未验证"。
2. **A8（零残留门）两条检索路径口径不一致**：`rg` 路径默认尊重 `.gitignore` 并跳过隐藏
   目录，退化 `grep -rIlF` 路径却不是。于是被 `.gitignore` 忽略的构建目录、或 `.agents/`
   这类隐藏目录里的残留符号，在装了 `rg` 的机器上"零命中"，在没有 `rg` 的机器上"有命中"。
   同一份代码，两种判定。
3. **A5（GitHub 适配器）覆盖面缺口**：适配器只跑 A4（`ci-verify.sh`），而 `ponygo status`
   才覆盖的判据（2.1 / 2.4 / 2.5 / 2.6 / 1.2 / 1.9b / 1.9c）在远端无人跑。

## Decision

采纳 ponygo 框架侧已修好的门禁资产，与模板逐字节一致地同步进本仓库：

1. **A1 早期 probe A6 并 fail-closed**：`adr-verify.sh` 在**任何跳过分支之前**探测
   `archive-verify.sh`；缺失、0 字节、或存在但不含判定标记（被换成 `exit 0` 的假门）一律
   拒绝提交，且拒绝文案点名 `A6` 与 `fail-closed`，让"不可判定"和"通过"在日志里长得不一样。
2. **A8 统一检索面**：`rg --no-ignore --fixed-strings --hidden`，退化 `grep -rIlF` 同口径；
   同时明确"空清单不构成验证"——清单是**删除发生时**由人登记的判定项，补一份空清单只是
   让门看起来在跑的仪式。
3. **A5 增加步骤 ②**：在 A4 之后跑 `ponygo status`；实例无 CLI 时输出 `::warning::` 并 exit 0
   （如实声明"未验证"，而不是伪装成通过）。退出码不变，聚合门语义不受影响。
4. **文档与自检同步**：`.meta/gates/README.md` 资产表与「承诺 → 非零退出命令」补齐上述三条；
   三个 spec 增加对应负样本（A2 38 asserts / A7 6 / A9 31），其中 A6 补回后的反向对照证明
   拒绝确由 A6 缺失造成，而非别的原因。

收据（本次全部 exit 0）：

```
bash .meta/gates/adr-verify.spec.sh      # 38 asserts，含 A6 缺失/清空/假门 → 拒绝
bash .meta/gates/archive-verify.spec.sh  # 6 asserts，5 类负样本 + 1 合法样本
bash .meta/gates/no-residue.spec.sh      # 31 asserts，含隐藏目录与被 ignore 目录命中必拒
```

## Alternatives considered

1. **A6 缺失时只 WARN 继续**：否决。删一个文件即可摘掉归档封印门，且 WARN 在 CI 日志里
   通常被折叠；"不可判定"必须与"通过"区分开，只有拒绝能做到。
2. **A6 探测保留在 A1 末尾（沿用原顺序）**：否决。空暂存区等跳过分支会在探测之前 return，
   门仍然可以被绕过；必须先 probe 再判。
3. **A8 只给退化 grep 路径加 `--no-ignore`/`--hidden`，`rg` 保留默认**：否决。两条路径必须
   同口径，否则同一工作树在两种环境下结论不同，而维护者无法从失败信息判断是哪条路径给出的。
4. **A5 步骤 ② 无 CLI 时直接失败**：否决。会把所有未安装 ponygo CLI 的实例（含 fork / CI
   缓存命中失败）判红，把一个"未验证"事实伪装成"验证失败"；`::warning::` + exit 0 更诚实，
   且被聚合门 `needs` 消费时不会误阻断发布。
5. **不写本条记录，按"模板同步属机械变更"豁免**：否决。门禁是承诺的可验形态，改它的判据
   就是改契约；豁免一次，后续就没有拒绝同类改动的依据。

## Consequences

- 提交路径上多了一次 A6 探测（毫秒级），代价换掉了"删文件即摘封印"这条后门。
- `no-residue` 现在会扫到 `.gitignore` 内的构建产物与隐藏目录，清单 pattern 一旦过宽，
  失败信息会变多；这是口径统一的必然代价，靠清单粒度控制而非放宽检索面。
- `ponygo status` 在无 CLI 的实例上只是 warning，因此远端层的判据覆盖依赖实例装 CLI；
  README 已注明"未验证 ≠ 通过"，review 时不应把绿 CI 当作这七条判据已覆盖的证据。
