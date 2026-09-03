use std::path::{Path, PathBuf};

/// Unified config discovery logic across CLI and Server:
/// 1. Explicit path (e.g. `--config /path/to/ponyllm.toml`): return immediately if provided.
/// 2. `PONYLLM_CONFIG` environment variable: return immediately if non-empty.
/// 3. Walk upwards from current working directory to filesystem root looking for `ponyllm.toml`.
/// 4. Global user configuration directory:
///    - `$HOME/.config/ponyllm/ponyllm.toml`
///    - `$HOME/.ponyllm.toml`
/// 5. Fallback to `ponyllm.toml` in CWD if none exists.
pub fn resolve_config_path(explicit: Option<&Path>) -> PathBuf {
    resolve_config_path_from(explicit, std::env::current_dir().ok().as_deref())
}

/// Internal helper allowing deterministic testing with simulated working directories.
pub fn resolve_config_path_from(explicit: Option<&Path>, cwd: Option<&Path>) -> PathBuf {
    // 1. Explicit path parameter
    if let Some(p) = explicit {
        return p.to_path_buf();
    }

    // 2. PONYLLM_CONFIG environment variable
    if let Ok(env_path) = std::env::var("PONYLLM_CONFIG") {
        let trimmed = env_path.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }

    // 3. Walk upwards from CWD
    if let Some(mut curr) = cwd {
        loop {
            let candidate = curr.join("ponyllm.toml");
            if candidate.is_file() {
                return candidate;
            }
            match curr.parent() {
                Some(parent) => curr = parent,
                None => break,
            }
        }
    }

    // 4. Global user configuration directory
    if let Some(user_home) = home_dir() {
        let xdg = user_home.join(".config").join("ponyllm").join("ponyllm.toml");
        if xdg.is_file() {
            return xdg;
        }
        let dot = user_home.join(".ponyllm.toml");
        if dot.is_file() {
            return dot;
        }
    }

    // 5. Default fallback to CWD / "ponyllm.toml"
    if let Some(curr) = cwd {
        curr.join("ponyllm.toml")
    } else {
        PathBuf::from("ponyllm.toml")
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}
