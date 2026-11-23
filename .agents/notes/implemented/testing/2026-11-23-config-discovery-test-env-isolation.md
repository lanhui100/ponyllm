# Agent Note: 配置发现测试的环境变量隔离（macOS CI 红）

Status: implemented

## Problem

CI `Test on macos-latest` 在 `-p ponyllm-core --test config_discovery_tests` 上失败：

```
test test_walk_upwards_from_child_directory ... FAILED
assertion `left == right` failed
  left: "/env/override/ponyllm.toml"
 right: "/var/folders/.../.tmpq3bEBH/ponyllm.toml"
```

`resolve_config_path_from` 读两个**进程全局**状态：`PONYLLM_CONFIG` 环境变量与当前工作目录。
测试文件里 `test_env_var_override` 用 `std::env::set_var` 写入该变量，而
`test_walk_upwards_from_child_directory` / `test_fallback_when_none_exists` 在同进程的
另一条测试线程上读它。cargo 默认并行跑测试函数，于是「写」可能赢下「读」。

linux 与 windows 只是恰好把写入和读取排开了，等价于靠调度器运气；macOS 换了调度顺序就暴露。
这不是产品代码缺陷，是测试之间的隐性共享状态。

## Decision

在 `crates/ponyllm-core/tests/config_discovery_tests.rs` 内引入进程内互斥
`static RESOLVER_ENV_LOCK: Mutex<()>`，四个测试函数统一持锁执行；锁中毒时用
`into_inner()` 取回 guard，避免一个失败用例把同文件其他用例一起挂死。
测试断言的语义不变，只把「调度器决定顺序」改成「文件内显式串行」。

约束：任何新增的、会读写 `PONYLLM_CONFIG` 或依赖 cwd 的用例，必须同样持这把锁；
新文件若也要操作该环境变量，需要复用同一把锁（跨文件时抽到共享 test support 模块），
否则竞态只是换了个文件名复发。

收据：

```
cargo test -p ponyllm-core --test config_discovery_tests -- --test-threads=8  # 连续 3 次 exit 0
```

## Alternatives considered

1. **只在 `test_env_var_override` 里加 `unsafe`/清理，不动其他用例**：否决。问题不在
   有没有清理，而在「写」与「读」并发发生；只清理写入端，读端仍会在写入窗口内读到他值。
2. **给整个测试二进制加 `--test-threads=1`**：否决。会拖慢同 crate 全部集成测试，
   而且把这个文件的隐性依赖外溢成全局约束；锁只需要覆盖真正共享环境的那四个用例。
3. **改用 `temp-env` / `serial_test` 依赖**：否决。为一个测试文件的隔离引入新依赖，
   维护面大于一把 `Mutex`；当前用例只有四个，显式锁更可读。
4. **删除 `test_env_var_override`**：否决。环境变量覆盖是 `resolve_config_path_from`
   的真实契约，删用例等于放弃覆盖，只是让 CI 变绿。
