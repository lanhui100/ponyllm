//! Tests for dashboard telemetry persistence and snapshot discovery.

use ponyllm_server::telemetry_snapshot::snapshot_path_for_config;
use std::path::{Path, PathBuf};

#[test]
fn test_snapshot_path_env_and_explicit_precedence() {
    // 1. Explicit path parameter has highest priority
    let explicit = Some("/tmp/explicit-telemetry.json");
    let path = snapshot_path_for_config(explicit, Some("/tmp/elog"), Some(Path::new("/etc/ponyllm/config.toml")));
    assert_eq!(path, Some(PathBuf::from("/tmp/explicit-telemetry.json")));

    // 2. Event log dir has next priority if explicit is None
    let path2 = snapshot_path_for_config(None, Some("/tmp/elog"), Some(Path::new("/etc/ponyllm/config.toml")));
    assert_eq!(path2, Some(PathBuf::from("/tmp/elog/telemetry-snapshot.json")));

    // 3. Fallback to config path parent
    let path3 = snapshot_path_for_config(None, None, Some(Path::new("/custom/dir/ponyllm.toml")));
    assert_eq!(path3, Some(PathBuf::from("/custom/dir/telemetry-snapshot.json")));
}

#[test]
fn test_snapshot_restore_roundtrip_preserves_history_and_tokens() {
    use ponyllm_core::telemetry::HourlyBucket;
    use ponyllm_server::telemetry_snapshot::{load_snapshot, save_snapshot_with_live_cycles, TelemetrySnapshot, SNAPSHOT_VERSION};
    use std::collections::{BTreeMap, HashMap};

    let temp_dir = tempfile::tempdir().expect("tempdir");
    let snap_path = temp_dir.path().join("telemetry-snapshot.json");

    let mut timeseries = BTreeMap::new();
    let mut b1 = HourlyBucket::default();
    b1.start_ms = 1700000000000;
    b1.total_requests = 42;
    b1.prompt_tokens = 1000;
    b1.completion_tokens = 500;
    timeseries.insert(1700000000000, b1);

    let snap = TelemetrySnapshot {
        version: SNAPSHOT_VERSION,
        saved_at_ms: 1700000005000,
        timeseries,
        ..Default::default()
    };

    save_snapshot_with_live_cycles(&snap_path, &snap, HashMap::new()).expect("save snapshot");

    let loaded = load_snapshot(&snap_path).expect("load snapshot");
    assert_eq!(loaded.timeseries.len(), 1);
    let loaded_b = loaded.timeseries.get(&1700000000000).expect("bucket");
    assert_eq!(loaded_b.total_requests, 42);
    assert_eq!(loaded_b.prompt_tokens, 1000);
    assert_eq!(loaded_b.completion_tokens, 500);
}
