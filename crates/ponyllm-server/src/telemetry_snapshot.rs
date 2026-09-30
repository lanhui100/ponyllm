//! Telemetry snapshot persistence: dashboard 最近统计周期落盘/启动恢复.
//!
//! 快照内容：时序小时桶（30天）、计数器、连通性（网关环+provider调用队列）、
//! 流节点EWMA、per-key 用量与周期用量归档。JSON单文件，原子写（tmp+rename），
//! 读失败返回 None 仅告警。
//!
//! 磁盘格式（schema v2，`unified-quota-metering-governance-kernel` M1 持久化部分）：
//! - 根对象即既有 [`TelemetrySnapshot`] 的全部字段（`version`/`saved_at_ms`/`timeseries`/…，
//!   旧代码可降级读取，未知键被忽略）；
//! - 额外注入 `schema_version`（磁盘格式版本；旧格式缺失 → 按 1 处理）与
//!   `key_usage_cycles`（每 key 周期用量归档：5h/周/月 四要素
//!   prompt/completion/cached/total/requests，数据源为既有 `CycleStats`）。
//! - 加载旧格式时**幂等迁移**：缺省字段补默认、历史数据全量保留，
//!   迁移前将原文件备份为 `<file>.bak-<ts>`，再落盘新格式。
//!
//! 不使用 `#[serde(flatten)]`：serde flatten 与 `BTreeMap<u64, _>`（timeseries
//! 数值键）冲突（"invalid type: string, expected u64"），故以 `serde_json::Value`
//! 做根级表示（见 [`TelemetrySnapshotFile::to_value`]/[`from_value`]）。

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use ponyllm_core::pool::usage::CycleStats;
use ponyllm_core::pool::NodeLatencySnapshot;
use ponyllm_core::telemetry::{
    HourlyBucket, MetricsCounterSnapshot, ProviderConnectivitySnapshot,
};
// 为 `serde_json::Error::custom` 提供 `serde::de::Error` trait 方法（匿名导入，无命名冲突）。
use serde::de::Error as _;

/// 兼容旧版写入器/读取器使用的 `version` 字段值（保持不变）。
pub const SNAPSHOT_VERSION: u32 = 1;
/// 当前磁盘格式 schema 版本：2 = 引入 `schema_version` 与 per-key 周期用量归档。
pub const SCHEMA_VERSION: u32 = 2;
/// 旧格式（无 `schema_version` 字段）的隐式 schema 版本。
pub const LEGACY_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TelemetrySnapshot {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub saved_at_ms: u64,
    #[serde(default)]
    pub timeseries: BTreeMap<u64, HourlyBucket>,
    #[serde(default)]
    pub metrics: MetricsCounterSnapshot,
    #[serde(default)]
    pub connectivity: HashMap<String, ProviderConnectivitySnapshot>,
    #[serde(default)]
    pub streams: HashMap<String, NodeLatencySnapshot>,
    #[serde(default)]
    pub key_usages: HashMap<String, ponyllm_core::pool::usage::KeyUsageStateSnapshot>,
}

fn default_version() -> u32 {
    SNAPSHOT_VERSION
}

impl Default for TelemetrySnapshot {
    fn default() -> Self {
        Self {
            version: SNAPSHOT_VERSION,
            saved_at_ms: 0,
            timeseries: BTreeMap::new(),
            metrics: MetricsCounterSnapshot::default(),
            connectivity: HashMap::new(),
            streams: HashMap::new(),
            key_usages: HashMap::new(),
        }
    }
}

/// 每 key 周期用量归档：5h / 周 / 月 三档四要素统计（来源为既有 [`CycleStats`]，
/// 见 2026-09-25 四要素计量 ADR）。新格式字段；旧格式缺省为空。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct KeyUsageCycleArchive {
    #[serde(default)]
    pub window_5h: CycleStats,
    #[serde(default)]
    pub weekly: CycleStats,
    #[serde(default)]
    pub monthly: CycleStats,
}

/// 磁盘快照文件表示（schema 版本 + 周期用量归档 + 既有快照主体）。
///
/// 根级 Value 表示：`snapshot` 字段直接落在 JSON 根（与旧格式同构），
/// `schema_version` / `key_usage_cycles` 为额外注入键。
#[derive(Debug, Clone, Default)]
pub struct TelemetrySnapshotFile {
    /// 磁盘格式版本；旧格式文件缺失该字段 → 按 [`LEGACY_SCHEMA_VERSION`] 处理。
    pub schema_version: u32,
    /// 每 key 周期用量归档（新格式；旧格式缺省为空）。
    pub key_usage_cycles: HashMap<String, KeyUsageCycleArchive>,
    /// 既有快照主体。
    pub snapshot: TelemetrySnapshot,
}

impl TelemetrySnapshotFile {
    /// 序列化为根级 JSON Value。
    pub fn to_value(&self) -> Result<serde_json::Value, serde_json::Error> {
        let mut value = serde_json::to_value(&self.snapshot)?;
        let obj = value
            .as_object_mut()
            .ok_or_else(|| serde_json::Error::custom("snapshot must serialize to an object"))?;
        obj.insert("schema_version".to_string(), serde_json::json!(self.schema_version));
        obj.insert(
            "key_usage_cycles".to_string(),
            serde_json::to_value(&self.key_usage_cycles)?,
        );
        Ok(value)
    }

    /// 从根级 JSON Value 解析：旧格式（缺 `schema_version` / `key_usage_cycles`）
    /// 按缺省补齐，快照主体忽略未知键。
    pub fn from_value(value: serde_json::Value) -> Result<Self, serde_json::Error> {
        let obj = value
            .as_object()
            .ok_or_else(|| serde_json::Error::custom("snapshot file root must be an object"))?;
        let schema_version = obj
            .get("schema_version")
            .and_then(|v| v.as_u64())
            .map(|n| n.min(u32::MAX as u64) as u32)
            .unwrap_or(LEGACY_SCHEMA_VERSION);
        let key_usage_cycles = obj
            .get("key_usage_cycles")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        let snapshot = serde_json::from_value(value)?;
        Ok(Self {
            schema_version,
            key_usage_cycles,
            snapshot,
        })
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn snapshot_path_for_config(
    explicit: Option<&str>,
    event_log_dir: Option<&str>,
    config_path: Option<&Path>,
) -> Option<std::path::PathBuf> {
    if let Some(p) = explicit {
        let t = p.trim();
        if !t.is_empty() {
            return Some(std::path::PathBuf::from(t));
        }
    }
    if let Some(dir) = event_log_dir {
        let t = dir.trim();
        if !t.is_empty() {
            return Some(std::path::PathBuf::from(t).join("telemetry-snapshot.json"));
        }
    }
    if let Some(cfg) = config_path {
        if let Some(parent) = cfg.parent() {
            if !parent.as_os_str().is_empty() {
                return Some(parent.join("telemetry-snapshot.json"));
            }
            return Some(std::path::PathBuf::from("telemetry-snapshot.json"));
        }
    }
    None
}

/// 读取磁盘快照文件（不做迁移）。
pub fn load_snapshot_file(path: &Path) -> Option<TelemetrySnapshotFile> {
    let content = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&content).ok()?;
    TelemetrySnapshotFile::from_value(value).ok()
}

/// 幂等迁移：旧格式（`schema_version` < [`SCHEMA_VERSION`]）→ 新格式。
///
/// 缺省字段已由解析时补齐，历史数据全量保留；返回 `true` 表示发生迁移
/// （调用方应负责备份原文件并持久化新格式）。
pub fn migrate_snapshot_file(file: &mut TelemetrySnapshotFile) -> bool {
    if file.schema_version >= SCHEMA_VERSION {
        return false;
    }
    // 旧格式无归档 → 保持空归档；其余字段解析时已按缺省补齐，只提升版本标记。
    file.schema_version = SCHEMA_VERSION;
    true
}

/// 迁移前备份原文件为 `<file>.bak-<ts>`。
fn backup_snapshot_file(path: &Path) -> std::io::Result<std::path::PathBuf> {
    let backup = path.with_extension(format!("json.bak-{}", now_ms()));
    std::fs::copy(path, &backup)?;
    Ok(backup)
}

/// 原子写（tmp+rename）。
fn write_snapshot_file(path: &Path, file: &TelemetrySnapshotFile) -> std::io::Result<()> {
    let value = file
        .to_value()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let content = serde_json::to_string(&value)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let tmp = path.with_extension(format!(
        "tmp.{}.{}",
        std::process::id(),
        now_ms() % 1_000_000
    ));
    std::fs::write(&tmp, content)?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Convert a rolling-window usage summary into the persisted [`CycleStats`]
/// shape (four-factor: prompt/completion/cached/total + requests). `count` is
/// the window's request count as the observation count; `avg_tokens` is
/// total / requests (0 when no requests). Source of the live 5h/周/月
/// per-key cycle archive (数据源为既有 `CycleStats` 结构).
pub fn window_usage_to_cycle_stats(
    usage: &ponyllm_core::pool::usage::WindowUsage,
) -> CycleStats {
    let requests = usage.requests;
    CycleStats {
        count: requests,
        prompt_tokens: usage.prompt_tokens,
        completion_tokens: usage.completion_tokens,
        cached_tokens: usage.cached_tokens,
        total_tokens: usage.total_tokens,
        requests,
        avg_tokens: if requests > 0 {
            usage.total_tokens / requests
        } else {
            0
        },
    }
}

/// 显式落盘（schema 版本 + 归档 + 快照主体），供归档写入方使用。
pub fn save_snapshot_file(path: &Path, file: &TelemetrySnapshotFile) -> std::io::Result<()> {
    write_snapshot_file(path, file)
}

pub fn load_snapshot(path: &Path) -> Option<TelemetrySnapshot> {
    let mut file = load_snapshot_file(path)?;
    if file.snapshot.version != SNAPSHOT_VERSION {
        tracing::warn!(
            "telemetry snapshot version mismatch (got {}, want {}), ignoring {:?}",
            file.snapshot.version,
            SNAPSHOT_VERSION,
            path
        );
        return None;
    }
    if migrate_snapshot_file(&mut file) {
        tracing::info!(
            "telemetry snapshot schema migrated ({} -> {}) at {:?}",
            LEGACY_SCHEMA_VERSION,
            SCHEMA_VERSION,
            path
        );
        match backup_snapshot_file(path) {
            Ok(backup) => {
                tracing::info!("telemetry snapshot original backed up to {:?}", backup);
                if let Err(e) = write_snapshot_file(path, &file) {
                    tracing::warn!("telemetry snapshot migration persist failed: {}", e);
                }
            }
            Err(e) => {
                // 备份失败则不覆盖原文件，保留旧数据（下次启动可再试迁移）。
                tracing::warn!("telemetry snapshot migration backup failed, original kept: {}", e);
            }
        }
    }
    Some(file.snapshot)
}

/// 周期保存：保留磁盘上已有的周期用量归档（读-改-写），并合并本次
/// live per-key CycleStats（5h/周/月，见 [`window_usage_to_cycle_stats`]）。
/// `live_cycles` 按 key id 覆盖归档；旧归档中本次未出现的 key 原样保留。
pub fn save_snapshot_with_live_cycles(
    path: &Path,
    snap: &TelemetrySnapshot,
    live_cycles: HashMap<String, KeyUsageCycleArchive>,
) -> std::io::Result<()> {
    let mut owned = snap.clone();
    owned.version = SNAPSHOT_VERSION;
    owned.saved_at_ms = now_ms();
    // 保留磁盘上已有的周期用量归档，避免周期性保存清空归档（读-改-写保持）。
    let mut cycles = load_snapshot_file(path)
        .map(|f| f.key_usage_cycles)
        .unwrap_or_default();
    for (key_id, archive) in live_cycles {
        cycles.insert(key_id, archive);
    }
    let file = TelemetrySnapshotFile {
        schema_version: SCHEMA_VERSION,
        key_usage_cycles: cycles,
        snapshot: owned,
    };
    write_snapshot_file(path, &file)
}

pub fn save_snapshot(path: &Path, snap: &TelemetrySnapshot) -> std::io::Result<()> {
    save_snapshot_with_live_cycles(path, snap, HashMap::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snapshot_path_prefers_explicit_then_event_log_then_config_dir() {
        let cfg = Path::new("/tmp/pony/ponyllm.toml");
        assert_eq!(
            snapshot_path_for_config(Some("/x/s.json"), Some("/elog"), Some(cfg)),
            Some(Path::new("/x/s.json").to_path_buf())
        );
        assert_eq!(
            snapshot_path_for_config(None, Some("/elog"), Some(cfg)),
            Some(Path::new("/elog/telemetry-snapshot.json").to_path_buf())
        );
        assert_eq!(
            snapshot_path_for_config(None, None, Some(cfg)),
            Some(Path::new("/tmp/pony/telemetry-snapshot.json").to_path_buf())
        );
        assert_eq!(snapshot_path_for_config(None, None, None), None);
    }

    #[test]
    fn test_snapshot_save_load_roundtrip() {
        let dir = std::env::temp_dir().join(format!(
            "ponyllm-snap-test-{}",
            now_ms()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("telemetry-snapshot.json");
        let mut snap = TelemetrySnapshot::default();
        snap.metrics.total_requests = 42;
        snap.timeseries.insert(
            1_700_000_000_000,
            HourlyBucket {
                start_ms: 1_700_000_000_000,
                total_requests: 2,
                ..Default::default()
            },
        );
        save_snapshot(&path, &snap).unwrap();
        let loaded = load_snapshot(&path).unwrap();
        assert_eq!(loaded.metrics.total_requests, 42);
        assert_eq!(loaded.timeseries.len(), 1);
        // 新格式落盘包含 schema 版本标记
        let file = load_snapshot_file(&path).unwrap();
        assert_eq!(file.schema_version, SCHEMA_VERSION);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 构造旧格式（无 schema_version / key_usage_cycles）文件 → 加载 →
    /// 断言数据不丢、schema 版本升为新值、原文件已备份、迁移幂等。
    #[test]
    fn test_legacy_snapshot_migration_preserves_data_and_bumps_schema() {
        let dir = std::env::temp_dir().join(format!("ponyllm-snap-migrate-{}", now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("telemetry-snapshot.json");

        let legacy = serde_json::json!({
            "version": 1,
            "saved_at_ms": 1_700_000_000_123u64,
            "timeseries": {
                "1700000000000": {
                    "start_ms": 1700000000000u64,
                    "total_requests": 7,
                    "prompt_tokens": 900,
                    "completion_tokens": 100
                }
            },
            "metrics": { "total_requests": 42, "successful_requests": 40 },
            "connectivity": {},
            "streams": {},
            "key_usages": {
                "key-1": { "slices": [], "cached_capacity": null, "last_probe": null }
            }
        });
        std::fs::write(&path, legacy.to_string()).unwrap();

        // 旧格式必须可加载
        let loaded = load_snapshot(&path).expect("legacy snapshot must load");
        assert_eq!(loaded.metrics.total_requests, 42);
        assert_eq!(loaded.timeseries.len(), 1);
        assert_eq!(loaded.key_usages.len(), 1);

        // 迁移已持久化：schema_version 升为新值，归档缺省为空，历史数据保留
        let migrated = load_snapshot_file(&path).expect("migrated file readable");
        assert_eq!(migrated.schema_version, SCHEMA_VERSION);
        assert!(migrated.key_usage_cycles.is_empty());
        assert_eq!(migrated.snapshot.metrics.total_requests, 42);
        assert_eq!(migrated.snapshot.timeseries.len(), 1);
        assert_eq!(migrated.snapshot.key_usages.len(), 1);

        // 迁移前原文件已备份为 .bak-<ts>
        let backups = count_backups(&dir);
        assert_eq!(backups, 1, "migration must back up the original file once");

        // 幂等：再次加载不产生新备份、不重复迁移
        let again = load_snapshot(&path).expect("second load must succeed");
        assert_eq!(again.metrics.total_requests, 42);
        assert_eq!(count_backups(&dir), 1, "idempotent: no second backup");

        std::fs::remove_dir_all(&dir).ok();
    }

    fn count_backups(dir: &std::path::Path) -> usize {
        std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("bak-"))
            .count()
    }

    /// 周期保存（`save_snapshot` 旧签名路径）不丢失磁盘上已有的周期用量归档。
    #[test]
    fn test_save_snapshot_preserves_key_usage_cycles() {
        let dir = std::env::temp_dir().join(format!("ponyllm-snap-cycles-{}", now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("telemetry-snapshot.json");

        // 先写入带归档的新格式文件
        let mut file = TelemetrySnapshotFile {
            schema_version: SCHEMA_VERSION,
            key_usage_cycles: HashMap::new(),
            snapshot: TelemetrySnapshot::default(),
        };
        let archive = KeyUsageCycleArchive {
            window_5h: CycleStats {
                count: 1,
                prompt_tokens: 100,
                completion_tokens: 50,
                cached_tokens: 20,
                total_tokens: 150,
                requests: 1,
                avg_tokens: 150,
            },
            weekly: CycleStats::default(),
            monthly: CycleStats::default(),
        };
        file.key_usage_cycles.insert("key-1".to_string(), archive);
        save_snapshot_file(&path, &file).unwrap();

        // 再走周期性保存路径 → 归档保留
        let mut snap = TelemetrySnapshot::default();
        snap.metrics.total_requests = 1;
        save_snapshot(&path, &snap).unwrap();

        let reloaded = load_snapshot_file(&path).unwrap();
        assert_eq!(reloaded.schema_version, SCHEMA_VERSION);
        let kept = reloaded.key_usage_cycles.get("key-1").expect("archive preserved");
        assert_eq!(kept.window_5h.total_tokens, 150);
        assert_eq!(kept.window_5h.prompt_tokens, 100);
        assert_eq!(kept.window_5h.cached_tokens, 20);
        assert_eq!(kept.window_5h.requests, 1);
        assert_eq!(reloaded.snapshot.metrics.total_requests, 1);

        std::fs::remove_dir_all(&dir).ok();
    }
}
