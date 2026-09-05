//! 网关进程生命周期管理：`stop` / `restart` 的 pidfile 机制。
//!
//! 设计约束：`upgrade` 只换磁盘二进制（无损），进程重启显式由用户触发，
//! 避免 CLI 自作主张杀进程掐断流式长连接。本模块只认“自己人”——pidfile
//! 与配置文件同目录（`ponyllm.pid`），不同 `--config` 即不同实例，互不干扰。

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

/// 配置文件同目录的 pidfile 路径：`<config-dir>/ponyllm.pid`。
pub fn pidfile_for_config(resolved_config: &Path) -> PathBuf {
    resolved_config
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
        .join("ponyllm.pid")
}

/// 后台 `serve` 子进程的日志追加路径：`<config-dir>/ponyllm-serve.log`。
pub fn logfile_for_config(resolved_config: &Path) -> PathBuf {
    resolved_config
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
        .join("ponyllm-serve.log")
}

/// 写入当前进程 pid；父目录不存在则创建。
pub fn write_pidfile(pidfile: &Path, pid: u32) -> Result<(), String> {
    if let Some(parent) = pidfile.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建 pidfile 目录失败: {}", e))?;
        }
    }
    std::fs::write(pidfile, pid.to_string()).map_err(|e| format!("写入 pidfile 失败: {}", e))
}

/// 读取 pidfile；文件缺失/内容非法返回 `None`（调用方按“无实例”处理）。
pub fn read_pidfile(pidfile: &Path) -> Option<u32> {
    std::fs::read_to_string(pidfile)
        .ok()?
        .trim()
        .parse::<u32>()
        .ok()
}

/// 跨平台进程存活探测。
pub fn process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        std::process::Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
    #[cfg(windows)]
    {
        let out = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {}", pid), "/FO", "CSV", "/NH"])
            .output();
        match out {
            Ok(o) if o.status.success() => {
                let text = String::from_utf8_lossy(&o.stdout);
                tasklist_csv_contains_pid(&text, pid)
            }
            _ => false,
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        false
    }
}

/// `tasklist /FO CSV /NH` 输出是否包含目标 pid（纯函数，可单测）。
/// 形如 `"ponyllm.exe","1234","Console","1","12,000 K"`，首列为映像名。
pub fn tasklist_csv_contains_pid(csv: &str, pid: u32) -> bool {
    let want = pid.to_string();
    csv.lines().any(|line| {
        let mut cols = line.split("\",\"");
        let _image = cols.next();
        match cols.next() {
            Some(pid_col) => pid_col.trim_matches('"').trim() == want,
            None => false,
        }
    })
}

/// 终止进程：先优雅（SIGTERM / taskkill），5 秒内未退出再强制。
/// 返回 `true` 表示进程已不在（本来就不在也算 `true`）。
pub async fn terminate_process(pid: u32) -> bool {
    if !process_alive(pid) {
        return true;
    }
    graceful_stop(pid);
    for _ in 0..50 {
        if !process_alive(pid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    force_stop(pid);
    for _ in 0..20 {
        if !process_alive(pid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    !process_alive(pid)
}

fn graceful_stop(pid: u32) {
    #[cfg(unix)]
    {
        let _ = std::process::Command::new("kill")
            .arg(pid.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
    }
}

fn force_stop(pid: u32) {
    #[cfg(unix)]
    {
        let _ = std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
    }
}

const MANUAL_STOP_HINT: &str = "未找到 ponyllm 的 pidfile（可能用其他方式启动）。请手动停止：前台进程按 Ctrl-C；后台进程用 `ps` 找到 ponyllm 后 `kill <PID>`；systemd 托管用 `sudo systemctl stop ponyllm`。";

/// 停止与该配置文件关联的网关进程。成功返回人话说明。
pub async fn stop_serve(config_path: Option<&str>) -> Result<String, String> {
    let resolved = crate::config::ConfigFile::resolve_path(config_path);
    let pidfile = pidfile_for_config(&resolved);
    let Some(pid) = read_pidfile(&pidfile) else {
        return Err(MANUAL_STOP_HINT.to_string());
    };
    if pid == std::process::id() {
        return Err(
            "pidfile 指向当前进程自己，拒绝自杀。请检查是否误用了同一配置文件。".to_string(),
        );
    }
    if !process_alive(pid) {
        let _ = std::fs::remove_file(&pidfile);
        return Ok(format!(
            " pidfile 中的进程 {} 已不在（陈旧 pidfile 已清理），无需停止。配置文件: {}",
            pid,
            resolved.display()
        ));
    }
    if terminate_process(pid).await {
        let _ = std::fs::remove_file(&pidfile);
        Ok(format!(
            "已停止网关进程 {}。配置文件: {}",
            pid,
            resolved.display()
        ))
    } else {
        Err(format!(
            "进程 {} 拒绝退出，请手动处理（`kill -9 {}` / 任务管理器），pidfile: {}",
            pid,
            pid,
            pidfile.display()
        ))
    }
}

/// `serve` 启动时的接管声明：pidfile 里有别的存活进程只告警不断行，
/// 随后写入自身 pid。返回告警文本（无告警为 `None`）。
pub fn claim_pidfile(resolved_config: &Path) -> Option<String> {
    let pidfile = pidfile_for_config(resolved_config);
    let me = std::process::id();
    if let Some(pid) = read_pidfile(&pidfile) {
        if pid != me && process_alive(pid) {
            return Some(format!(
                "⚠️ 检测到另一个 ponyllm 进程 {} 仍在使用同一配置文件（{}），双实例会互相抢端口/覆盖 pidfile，建议先 `ponyllm stop` 再启动。本次继续启动并接管 pidfile。",
                pid,
                resolved_config.display()
            ));
        }
    }
    if let Err(e) = write_pidfile(&pidfile, me) {
        return Some(format!("⚠️ {}", e));
    }
    None
}

/// 退出时尽力清理 pidfile（kill -9 等场景清不掉属预期，由下次启动的存活探测处理）。
pub fn release_pidfile(resolved_config: &Path) {
    let _ = std::fs::remove_file(pidfile_for_config(resolved_config));
}

/// 后台拉起新的 `serve` 进程并返回其 pid。`serve_args` 为透传给 `serve` 的参数
///（如 `--config/--bind/--port/--api-key/--retries` 的显式覆盖）。
pub fn spawn_detached_serve(resolved_config: &Path, serve_args: &[String]) -> Result<u32, String> {
    let exe = std::env::current_exe().map_err(|e| format!("获取当前可执行文件路径失败: {}", e))?;
    let logfile = logfile_for_config(resolved_config);
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&logfile)
        .map_err(|e| format!("打开日志文件失败 ({}): {}", logfile.display(), e))?;
    let err_log = log
        .try_clone()
        .map_err(|e| format!("复用日志句柄失败: {}", e))?;

    let mut cmd = std::process::Command::new(exe);
    cmd.arg("serve").args(serve_args);
    cmd.stdin(Stdio::null()).stdout(log).stderr(err_log);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x00000008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(unix)]
    {
        // 父进程退出后子进程被 init 接管继续在后台运行；
        // 生产环境建议用 systemd 托管（见 README），本命令面向开发机一键重启。
    }
    let child = cmd
        .spawn()
        .map_err(|e| format!("拉起后台 serve 失败: {}", e))?;
    Ok(child.id())
}

/// 读取文件末尾至多 `max_lines` 行文本（用于捕获进程崩溃时的 stderr/stdout 详情）。
pub fn read_tail_lines(path: &Path, max_lines: usize) -> String {
    let Ok(content) = std::fs::read_to_string(path) else {
        return String::new();
    };
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return String::new();
    }
    let start = lines.len().saturating_sub(max_lines);
    lines[start..].join("\n")
}

/// 探测目标网络地址是否已被占用（尝试独占 bind 探测）。
pub fn is_addr_in_use(bind_addr: &str) -> bool {
    let addr = if bind_addr.starts_with(':') {
        format!("0.0.0.0{}", bind_addr)
    } else {
        bind_addr.to_string()
    };
    match std::net::TcpListener::bind(&addr) {
        Ok(l) => {
            drop(l);
            false
        }
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => true,
        Err(_) => false,
    }
}

/// 等待后台新实例存活并准备就绪。若进程在探测窗内夭折，读取日志尾部报错并返回 Err。
pub async fn wait_process_alive_and_ready(
    pid: u32,
    logfile: &Path,
    timeout: Duration,
) -> Result<(), String> {
    let start = std::time::Instant::now();
    let poll_interval = Duration::from_millis(100);
    while start.elapsed() < timeout {
        if !process_alive(pid) {
            let tail = read_tail_lines(logfile, 15);
            let detail = if tail.trim().is_empty() {
                "（日志文件为空，进程未输出错误即退出）".to_string()
            } else {
                tail
            };
            return Err(format!(
                "后台网关实例 (pid: {}) 启动后立即异常退出！\n错误日志 ({}):\n{}\n请检查目标端口是否被占用，或配置参数是否正确。",
                pid,
                logfile.display(),
                detail
            ));
        }
        tokio::time::sleep(poll_interval).await;
    }
    if !process_alive(pid) {
        let tail = read_tail_lines(logfile, 15);
        return Err(format!(
            "后台网关实例 (pid: {}) 异常退出！\n错误日志 ({}):\n{}",
            pid,
            logfile.display(),
            tail
        ));
    }
    Ok(())
}

/// 重启：先停（无实例也继续），再以后台方式拉起，覆盖 pidfile。
pub async fn restart_serve(
    config: Option<&str>,
    bind: Option<String>,
    address: Option<String>,
    port: Option<u16>,
    api_key: Option<String>,
    retries: Option<usize>,
) -> Result<String, String> {
    let resolved = crate::config::ConfigFile::resolve_path(config);
    let cfg = crate::config::ConfigFile::load_or_default(resolved.to_str()).ok();
    let target_bind = bind
        .clone()
        .or_else(|| {
            address
                .clone()
                .and_then(|a| port.map(|p| format!("{}:{}", a, p)))
        })
        .or_else(|| cfg.as_ref().map(|c| c.gateway.bind.clone()))
        .unwrap_or_else(|| "127.0.0.1:8080".to_string());

    let stop_note = match stop_serve(config).await {
        Ok(msg) => msg,
        Err(e) if e == MANUAL_STOP_HINT => {
            if is_addr_in_use(&target_bind) {
                let (host, p_str) = target_bind.split_once(':').unwrap_or(("127.0.0.1", "8080"));
                let probe_host = if host == "0.0.0.0" { "127.0.0.1" } else { host };
                let probe_url = format!("http://{}:{}/health", probe_host, p_str);
                let client = reqwest::Client::builder()
                    .timeout(Duration::from_millis(800))
                    .build()
                    .ok();
                let is_pony = if let Some(cli) = client {
                    match cli.get(&probe_url).send().await {
                        Ok(resp) if resp.status().is_success() => resp
                            .json::<serde_json::Value>()
                            .await
                            .ok()
                            .and_then(|j| {
                                j.get("service")
                                    .and_then(|s| s.as_str())
                                    .map(|s| s == "ponyllm")
                            })
                            .unwrap_or(false),
                        _ => false,
                    }
                } else {
                    false
                };

                if is_pony {
                    return Err(format!(
                        "未找到 pidfile，且检测到目标地址 {} 已被运行中的旧版 ponyllm 实例占用！\n👉 该实例可能系旧版本（无 pidfile 机制）启动。为防止新实例端口冲突崩溃，已终止重启。\n👉 请先手动停止旧进程（如 kill 或 pkill ponyllm）后，再执行 `ponyllm restart`。",
                        target_bind
                    ));
                } else {
                    return Err(format!(
                        "未找到 pidfile，且检测到目标地址 {} 已被占用！无法拉起新实例。\n👉 请先释放占用该端口的进程后，再执行 `ponyllm restart`。",
                        target_bind
                    ));
                }
            }
            "未发现 pidfile 管理的旧实例，以冷启动方式拉起新实例。".to_string()
        }
        Err(e) => return Err(e),
    };

    let mut args: Vec<String> = Vec::new();
    if let Some(c) = config {
        args.push("--config".to_string());
        args.push(c.to_string());
    }
    if let Some(b) = bind {
        args.push("--bind".to_string());
        args.push(b);
    }
    if let Some(a) = address {
        args.push("--address".to_string());
        args.push(a);
    }
    if let Some(p) = port {
        args.push("--port".to_string());
        args.push(p.to_string());
    }
    if let Some(k) = api_key {
        args.push("--api-key".to_string());
        args.push(k);
    }
    if let Some(r) = retries {
        args.push("--retries".to_string());
        args.push(r.to_string());
    }

    // 小睡一拍让出端口，避免旧进程尚在释放监听时新进程绑定失败。
    tokio::time::sleep(Duration::from_millis(500)).await;
    let pid = spawn_detached_serve(&resolved, &args)?;
    let pidfile = pidfile_for_config(&resolved);
    let logfile = logfile_for_config(&resolved);

    // 关键防欺诈机制：探测新实例是否在启动阶段夭折（例如端口冲突、配置损坏）
    if let Err(e) = wait_process_alive_and_ready(pid, &logfile, Duration::from_millis(1500)).await {
        let _ = std::fs::remove_file(&pidfile);
        return Err(e);
    }

    write_pidfile(&pidfile, pid)?;
    Ok(format!(
        "旧实例：{}新实例 pid {} 已在后台拉起并就绪运行，日志追加至 {}。请用 `ponyllm status` 确认版本与 Key 状态。",
        stop_note,
        pid,
        logfile.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pidfile_and_logfile_derive_from_config_dir() {
        let cfg = Path::new("/etc/ponyllm/ponyllm.toml");
        assert_eq!(
            pidfile_for_config(cfg),
            PathBuf::from("/etc/ponyllm/ponyllm.pid")
        );
        assert_eq!(
            logfile_for_config(cfg),
            PathBuf::from("/etc/ponyllm/ponyllm-serve.log")
        );
        let bare = Path::new("ponyllm.toml");
        assert_eq!(pidfile_for_config(bare), PathBuf::from("./ponyllm.pid"));
    }

    #[test]
    fn test_pidfile_roundtrip_and_garbage() {
        let tmp = tempfile::tempdir().unwrap();
        let pidfile = tmp.path().join("ponyllm.pid");
        assert_eq!(read_pidfile(&pidfile), None);
        write_pidfile(&pidfile, 4242).unwrap();
        assert_eq!(read_pidfile(&pidfile), Some(4242));
        std::fs::write(&pidfile, "not-a-pid\n").unwrap();
        assert_eq!(read_pidfile(&pidfile), None);
    }

    #[test]
    fn test_tasklist_csv_contains_pid() {
        let csv = "\"ponyllm.exe\",\"4242\",\"Console\",\"1\",\"12,000 K\"\r\n\"other.exe\",\"42420\",\"Console\",\"1\",\"8,000 K\"";
        assert!(tasklist_csv_contains_pid(csv, 4242));
        assert!(!tasklist_csv_contains_pid(csv, 424));
        assert!(!tasklist_csv_contains_pid(csv, 42420 + 1));
        assert!(!tasklist_csv_contains_pid(
            "INFO: No tasks are running",
            4242
        ));
    }

    #[test]
    fn test_current_process_is_alive() {
        assert!(process_alive(std::process::id()));
        assert!(!process_alive(4194304));
    }

    #[test]
    fn test_read_tail_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let logfile = tmp.path().join("test.log");
        let content = (1..=20)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&logfile, content).unwrap();

        let tail = read_tail_lines(&logfile, 3);
        assert_eq!(tail, "line 18\nline 19\nline 20");

        let empty = tmp.path().join("non_exist.log");
        assert_eq!(read_tail_lines(&empty, 5), "");
    }

    #[test]
    fn test_is_addr_in_use() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        assert!(is_addr_in_use(&addr));
        drop(listener);
        assert!(!is_addr_in_use(&addr));
    }

    #[tokio::test]
    async fn test_wait_process_alive_and_ready_aborts_on_dead_pid() {
        let tmp = tempfile::tempdir().unwrap();
        let logfile = tmp.path().join("crash.log");
        std::fs::write(
            &logfile,
            "Error: Os { code: 98, kind: AddrInUse, message: \"Address already in use\" }",
        )
        .unwrap();

        // 4194304 is guaranteed not alive
        let res = wait_process_alive_and_ready(4194304, &logfile, Duration::from_millis(300)).await;
        assert!(res.is_err());
        let err = res.unwrap_err();
        assert!(err.contains("4194304"));
        assert!(err.contains("Address already in use"));
    }
}
