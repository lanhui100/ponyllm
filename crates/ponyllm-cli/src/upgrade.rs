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

/// Download release asset with multi-mirror fallback and retries
async fn download_asset_with_retry(
    client: &reqwest::Client,
    primary_url: &str,
    user_agent: &str,
) -> Result<Vec<u8>, String> {
    let candidate_urls = vec![
        primary_url.to_string(),
        format!("https://ghfast.top/{}", primary_url),
        format!("https://ghproxy.net/{}", primary_url),
    ];

    let mut last_err = String::new();

    for (attempt, url) in candidate_urls.iter().enumerate() {
        if attempt > 0 {
            println!("--> [备选加速通道] 正在尝试备用下载源 #{}: {}", attempt, url);
        }
        match client
            .get(url)
            .header("User-Agent", user_agent)
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                match resp.bytes().await {
                    Ok(b) if !b.is_empty() => return Ok(b.to_vec()),
                    Ok(_) => last_err = "下载到空数据包".to_string(),
                    Err(e) => last_err = format!("读取数据流失败: {e}"),
                }
            }
            Ok(resp) => {
                last_err = format!("HTTP 状态异常: {}", resp.status());
            }
            Err(e) => {
                last_err = format!("网络连接错误: {e}");
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    Err(format!("下载资产失败（已尝试主源及备用加速镜像）: {last_err}"))
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

    println!("========================================================");
    println!("  ponyllm 自动升级检测");
    println!("  当前安装版本: v{}", current_version);
    println!("  当前系统架构: {}-{} (目标资产: {})", std::env::consts::OS, std::env::consts::ARCH, asset_name);
    println!("========================================================");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    println!("--> 正在查询 GitHub Releases 最新版本信息...");
    let release = fetch_release_info(&client, target_version.as_deref()).await?;
    let release_tag = release.tag_name.trim();
    let is_newer = is_newer_version(current_version, release_tag);

    println!("--> 远程发布版本: {}", release_tag);

    if check_only {
        if is_newer {
            println!("--> [发现新版本] 可升级至: {} (发布详情: {})", release_tag, release.html_url);
            println!("    运行 'ponyllm upgrade' 即可原地一键升级。");
        } else {
            println!("--> [已是最新版本] 当前版本 v{} 无需升级。", current_version);
        }
        return Ok(());
    }

    if !is_newer && !force && target_version.is_none() {
        println!("--> [已是最新版本] 当前安装的 v{} 已是最新发布版本。", current_version);
        println!("    如需强制重装当前版本，请添加 '--force' 参数：ponyllm upgrade --force");
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
        "--> 找到匹配平台资产: {} ({:.2} MB)",
        matching_asset.name,
        matching_asset.size as f64 / 1_048_576.0
    );
    println!("--> 资源下载地址: {}", matching_asset.browser_download_url);

    if dry_run {
        println!("--> [Dry Run] 模拟运行已完成，未下载或修改任何本地文件。");
        return Ok(());
    }

    println!("--> 正在流式下载资产包...");
    let user_agent = format!("ponyllm/{}", current_version);
    let archive_bytes = download_asset_with_retry(&client, &matching_asset.browser_download_url, &user_agent).await?;
    println!("--> 下载完成 ({} 字节)，正在解压校验...", archive_bytes.len());

    let temp_dir = tempfile::tempdir()?;
    let extracted_binary = if is_zip {
        extract_zip(&archive_bytes, binary_name, temp_dir.path())?
    } else {
        extract_targz(&archive_bytes, binary_name, temp_dir.path())?
    };

    println!("--> 解压完成: {}", extracted_binary.display());
    println!("--> 正在执行可执行文件原地自替换...");

    let replaced_path = perform_self_replacement(&extracted_binary)?;

    println!("========================================================");
    println!("  ponyllm 升级成功！");
    println!("  安装路径: {}", replaced_path.display());
    println!("  版本变更: v{} -> {}", current_version, release_tag);
    println!("========================================================");
    println!("  运行 'ponyllm --version' 验证更新后的版本。");

    Ok(())
}
