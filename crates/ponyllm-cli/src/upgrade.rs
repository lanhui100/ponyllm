//! Self-upgrade manager for ponyllm CLI
//! Fetches release metadata from GitHub Releases, downloads target binary assets,
//! and performs atomic cross-platform self-replacement.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ReleaseInfo {
    pub tag_name: String,
    pub name: Option<String>,
    pub html_url: String,
    pub body: Option<String>,
    #[serde(default)]
    pub assets: Vec<AssetInfo>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct AssetInfo {
    pub name: String,
    pub browser_download_url: String,
    #[serde(default)]
    pub size: u64,
}

/// Detect target platform asset and binary filename.
/// Returns (asset_name, binary_name, is_zip).
pub fn detect_target_asset_name() -> Result<(&'static str, &'static str, bool), String> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    match (os, arch) {
        ("windows", "x86_64") => Ok(("ponyllm-windows-x86_64.zip", "ponyllm.exe", true)),
        ("linux", "x86_64") => Ok(("ponyllm-linux-x86_64.tar.gz", "ponyllm", false)),
        ("linux", "aarch64") => Ok(("ponyllm-linux-aarch64.tar.gz", "ponyllm", false)),
        ("macos", "x86_64") => Ok(("ponyllm-macos-x86_64.tar.gz", "ponyllm", false)),
        ("macos", "aarch64") => Ok(("ponyllm-macos-aarch64.tar.gz", "ponyllm", false)),
        _ => Err(format!("Unsupported platform/architecture: {os}-{arch}")),
    }
}

/// Parse a version string (e.g. "0.2.1" or "v0.2.1") into (major, minor, patch)
pub fn parse_version_triplet(v: &str) -> Option<(u64, u64, u64)> {
    let clean = v.trim().trim_start_matches('v').trim_start_matches('V');
    let parts: Vec<&str> = clean.split('.').collect();
    if parts.len() >= 3 {
        let major = parts[0].parse::<u64>().ok()?;
        let minor = parts[1].parse::<u64>().ok()?;
        let patch = parts[2].split('-').next()?.parse::<u64>().ok()?;
        Some((major, minor, patch))
    } else {
        None
    }
}

/// Check if `target` version is strictly newer than `current` version.
pub fn is_newer_version(current: &str, target: &str) -> bool {
    if let (Some(cur), Some(tgt)) = (parse_version_triplet(current), parse_version_triplet(target)) {
        tgt > cur
    } else {
        let c = current.trim().trim_start_matches('v').trim_start_matches('V');
        let t = target.trim().trim_start_matches('v').trim_start_matches('V');
        c != t
    }
}

/// Fetch release information from GitHub Releases API
pub async fn fetch_release_info(
    client: &reqwest::Client,
    target_version: Option<&str>,
) -> Result<ReleaseInfo, String> {
    let url = match target_version {
        Some(ver) => {
            let tag = if ver.starts_with('v') || ver.starts_with('V') {
                ver.to_string()
            } else {
                format!("v{ver}")
            };
            format!("https://api.github.com/repos/lanhui100/ponyllm/releases/tags/{tag}")
        }
        None => "https://api.github.com/repos/lanhui100/ponyllm/releases/latest".to_string(),
    };

    let user_agent = format!("ponyllm/{}", env!("CARGO_PKG_VERSION"));
    let res = client
        .get(&url)
        .header("User-Agent", user_agent)
        .header("Accept", "application/vnd.github.v3+json")
        .send()
        .await
        .map_err(|e| format!("Failed to connect to GitHub Releases API: {e}"))?;

    let status = res.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(format!("Release version not found at URL: {url}"));
    }
    if status == reqwest::StatusCode::FORBIDDEN {
        return Err("GitHub API rate limit exceeded. Please try again later or use install script.".to_string());
    }
    if !status.is_success() {
        return Err(format!("GitHub API returned HTTP {status}"));
    }

    let release = res
        .json::<ReleaseInfo>()
        .await
        .map_err(|e| format!("Failed to parse GitHub release payload: {e}"))?;

    Ok(release)
}

/// Extract zip archive in-memory and locate the target binary
pub fn extract_zip(bytes: &[u8], target_binary_name: &str, dest_dir: &Path) -> Result<PathBuf, String> {
    let reader = std::io::Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(reader)
        .map_err(|e| format!("Failed to parse zip archive: {e}"))?;

    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(|e| format!("Corrupted zip entry {i}: {e}"))?;
        let file_name = match file.enclosed_name() {
            Some(path) => path.to_owned(),
            None => continue,
        };

        if file_name.file_name().and_then(|s| s.to_str()) == Some(target_binary_name) {
            let outpath = dest_dir.join(target_binary_name);
            let mut outfile = std::fs::File::create(&outpath)
                .map_err(|e| format!("Failed to create destination file {}: {e}", outpath.display()))?;
            std::io::copy(&mut file, &mut outfile)
                .map_err(|e| format!("Failed to extract binary from zip: {e}"))?;
            return Ok(outpath);
        }
    }

    Err(format!("Binary '{target_binary_name}' not found inside zip archive"))
}

/// Extract tar.gz archive in-memory and locate the target binary
pub fn extract_targz(bytes: &[u8], target_binary_name: &str, dest_dir: &Path) -> Result<PathBuf, String> {
    let tar_gz = std::io::Cursor::new(bytes);
    let tar = flate2::read::GzDecoder::new(tar_gz);
    let mut archive = tar::Archive::new(tar);

    let entries = archive
        .entries()
        .map_err(|e| format!("Failed to read tar.gz archive: {e}"))?;

    for entry in entries {
        let mut entry = entry.map_err(|e| format!("Corrupted tar entry: {e}"))?;
        let path = entry
            .path()
            .map_err(|e| format!("Invalid tar entry path: {e}"))?
            .to_path_buf();

        if path.file_name().and_then(|s| s.to_str()) == Some(target_binary_name) {
            let outpath = dest_dir.join(target_binary_name);
            entry
                .unpack(&outpath)
                .map_err(|e| format!("Failed to unpack binary from tar.gz: {e}"))?;
            return Ok(outpath);
        }
    }

    Err(format!("Binary '{target_binary_name}' not found inside tar.gz archive"))
}

/// Atomically replace current executable with the new binary file
pub fn perform_self_replacement(new_binary_path: &Path) -> Result<PathBuf, String> {
    let current_exe = std::env::current_exe()
        .map_err(|e| format!("Failed to determine current executable path: {e}"))?;

    let canonical_exe = current_exe.canonicalize().unwrap_or_else(|_| current_exe.clone());

    #[cfg(windows)]
    {
        // On Windows, a running executable cannot be directly written to,
        // but it CAN be renamed while running.
        let old_exe = canonical_exe.with_extension("exe.old");
        if old_exe.exists() {
            let _ = std::fs::remove_file(&old_exe);
        }

        std::fs::rename(&canonical_exe, &old_exe)
            .map_err(|e| format!("Failed to rename running binary to backup ({:?}): {e}", old_exe))?;

        if let Err(e) = std::fs::copy(new_binary_path, &canonical_exe) {
            // Rollback on failure
            let _ = std::fs::rename(&old_exe, &canonical_exe);
            return Err(format!("Failed to copy new binary into target destination: {e}"));
        }

        // Try cleaning up the old file
        let _ = std::fs::remove_file(&old_exe);
    }

    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = std::fs::metadata(new_binary_path) {
            let mut perms = metadata.permissions();
            perms.set_mode(0o755);
            let _ = std::fs::set_permissions(new_binary_path, perms);
        }

        let parent_dir = canonical_exe.parent().unwrap_or_else(|| Path::new("."));
        let temp_dst = tempfile::Builder::new()
            .prefix(".ponyllm-upgrade-")
            .tempfile_in(parent_dir)
            .map_err(|e| format!("Failed to create temporary file in {:?}: {e}", parent_dir))?;

        let temp_path = temp_dst.into_temp_path();
        std::fs::copy(new_binary_path, &temp_path)
            .map_err(|e| format!("Failed to copy new binary to temp file: {e}"))?;

        if let Ok(metadata) = std::fs::metadata(&temp_path) {
            let mut perms = metadata.permissions();
            perms.set_mode(0o755);
            let _ = std::fs::set_permissions(&temp_path, perms);
        }

        std::fs::rename(&temp_path, &canonical_exe)
            .map_err(|e| format!("Failed to atomically replace executable {:?}: {e}", canonical_exe))?;
    }

    Ok(canonical_exe)
}

/// Download release asset with multi-mirror fallback and retries.
/// API 查询用短超时，下载用独立长超时：慢链路（实测约 20KB/s，6.5MB 需 5 分钟以上）
/// 下 30s client 必超时。分块流式读取并按 MB 打印进度；聚合每次尝试的错误，
/// 避免单次失败掩盖主因。
pub const API_TIMEOUT_SECS: u64 = 30;
pub const DOWNLOAD_TIMEOUT_SECS: u64 = 600;

/// 按优先级构造候选下载地址：主源优先，镜像仅作其它网络下的 fallback。
pub fn build_candidate_urls(primary_url: &str) -> Vec<String> {
    vec![
        primary_url.to_string(),
        format!("https://ghfast.top/{}", primary_url),
        format!("https://ghproxy.net/{}", primary_url),
    ]
}

pub fn build_download_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(DOWNLOAD_TIMEOUT_SECS))
        .build()
        .map_err(|e| format!("Failed to build download client: {e}"))
}

fn format_mb(bytes: u64) -> String {
    format!("{:.1}MB", bytes as f64 / 1_048_576.0)
}

fn format_speed(bytes_per_sec: f64) -> String {
    if bytes_per_sec >= 1_048_576.0 {
        format!("{:.1}MB/s", bytes_per_sec / 1_048_576.0)
    } else if bytes_per_sec >= 1024.0 {
        format!("{:.0}KB/s", bytes_per_sec / 1024.0)
    } else {
        format!("{:.0}B/s", bytes_per_sec)
    }
}

/// 单行进度条（纯函数）：`下载 [██████░░░░]  51% 3.3/6.5MB 20KB/s`
pub fn render_progress_line(downloaded: u64, total: u64, elapsed_secs: f64) -> String {
    let pct = if total > 0 {
        ((downloaded as f64 / total as f64) * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };
    let filled = (pct / 5.0).round() as usize;
    let bar: String = (0..20).map(|i| if i < filled { '█' } else { '░' }).collect();
    let speed = if elapsed_secs > 0.0 {
        downloaded as f64 / elapsed_secs
    } else {
        0.0
    };
    format!(
        "下载 [{}] {:>3.0}% {}/{} {}",
        bar,
        pct,
        format_mb(downloaded),
        if total > 0 { format_mb(total) } else { "?".to_string() },
        format_speed(speed)
    )
}

async fn download_asset_with_retry(
    client: &reqwest::Client,
    primary_url: &str,
    user_agent: &str,
) -> Result<Vec<u8>, String> {
    let candidate_urls = build_candidate_urls(primary_url);

    let mut attempt_errors: Vec<String> = Vec::new();

    for (attempt, url) in candidate_urls.iter().enumerate() {
        if attempt > 0 {
            eprintln!("备选源 #{} 不可用，换下一个", attempt);
        }
        let result: Result<Vec<u8>, String> = async {
            let resp = client
                .get(url)
                .header("User-Agent", user_agent)
                .send()
                .await
                .map_err(|e| format!("网络连接错误: {e}"))?;
            if !resp.status().is_success() {
                return Err(format!("HTTP 状态异常: {}", resp.status()));
            }
            let total = resp.content_length().unwrap_or(0);
            let mut buf = Vec::with_capacity(total.min(64 * 1024 * 1024) as usize);
            let mut stream = resp.bytes_stream();
            use futures_util::StreamExt;
            use std::io::{IsTerminal, Write as _};
            let interactive = std::io::stdout().is_terminal();
            let started = std::time::Instant::now();
            let mut last_pct = u64::MAX;
            let mut last_print = std::time::Instant::now();
            // TTY 原地刷新单行进度条；非 TTY 每 25% 一行，避免日志刷屏
            let mut paint = |downloaded: u64, force: bool| {
                let pct = if total > 0 {
                    ((downloaded as f64 / total as f64) * 100.0).clamp(0.0, 100.0) as u64
                } else {
                    0
                };
                let due = last_print.elapsed() >= std::time::Duration::from_millis(500);
                if interactive {
                    if force || pct != last_pct || due {
                        print!(
                            "\r{}",
                            render_progress_line(downloaded, total, started.elapsed().as_secs_f64())
                        );
                        let _ = std::io::stdout().flush();
                        last_pct = pct;
                        last_print = std::time::Instant::now();
                    }
                } else if force || pct / 25 != last_pct / 25 {
                    println!(
                        "{}",
                        render_progress_line(downloaded, total, started.elapsed().as_secs_f64())
                    );
                    last_pct = pct;
                }
            };
            paint(0, true);
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|e| format!("读取数据流失败: {e}"))?;
                buf.extend_from_slice(&chunk);
                paint(buf.len() as u64, false);
            }
            paint(buf.len() as u64, true);
            if interactive {
                println!();
            }
            if buf.is_empty() {
                return Err("下载到空数据包".to_string());
            }
            Ok(buf)
        }
        .await;

        match result {
            Ok(b) => return Ok(b),
            Err(e) => {
                attempt_errors.push(format!("[{}] {}", url, e));
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    Err(format!(
        "下载资产失败（已尝试主源及备用加速镜像）：\n  {}",
        attempt_errors.join("\n  ")
    ))
}

/// Orchestrate the entire upgrade workflow
pub async fn run_upgrade(
    check_only: bool,
    force: bool,
    dry_run: bool,
    target_version: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let current_version = env!("CARGO_PKG_VERSION");
    let (asset_name, binary_name, is_zip) = detect_target_asset_name()
        .map_err(|e| format!("Platform detection error: {e}"))?;

    println!(
        "ponyllm 升级检查：当前 v{}（{}-{}）",
        current_version,
        std::env::consts::OS,
        std::env::consts::ARCH
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(API_TIMEOUT_SECS))
        .build()?;

    println!("查询最新版本...");
    let release = fetch_release_info(&client, target_version.as_deref()).await?;
    let release_tag = release.tag_name.trim();
    let is_newer = is_newer_version(current_version, release_tag);

    println!("最新版本：{}", release_tag);

    if check_only {
        if is_newer {
            println!("发现新版本 {}，运行 `ponyllm upgrade` 一键升级。", release_tag);
        } else {
            println!("已是最新版本，无需升级。");
        }
        return Ok(());
    }

    if !is_newer && !force && target_version.is_none() {
        println!("已是最新版本 v{}，无需升级（`--force` 可强制重装）。", current_version);
        return Ok(());
    }

    // Match platform asset
    let matching_asset = release
        .assets
        .iter()
        .find(|a| a.name == asset_name)
        .ok_or_else(|| {
            format!(
                "未在 Release {} 中找到匹配当前平台的资产文件 '{}'。\n可用资产列表: {:?}",
                release_tag,
                asset_name,
                release.assets.iter().map(|a| &a.name).collect::<Vec<_>>()
            )
        })?;

    println!(
        "资产：{}（{}），开始下载...",
        matching_asset.name,
        format_mb(matching_asset.size)
    );

    if dry_run {
        println!("Dry Run：仅检查，不下载不修改。");
        return Ok(());
    }

    let user_agent = format!("ponyllm/{}", current_version);
    let download_client =
        build_download_client().map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
    let archive_bytes =
        download_asset_with_retry(&download_client, &matching_asset.browser_download_url, &user_agent).await?;
    println!("下载完成，解压校验...");

    let temp_dir = tempfile::tempdir()?;
    let extracted_binary = if is_zip {
        extract_zip(&archive_bytes, binary_name, temp_dir.path())?
    } else {
        extract_targz(&archive_bytes, binary_name, temp_dir.path())?
    };

    println!("解压通过，正在替换...");
    let replaced_path = perform_self_replacement(&extracted_binary)?;

    println!(
        "升级成功：v{} -> {}（{}）",
        current_version,
        release_tag,
        replaced_path.display()
    );

    Ok(())
}
