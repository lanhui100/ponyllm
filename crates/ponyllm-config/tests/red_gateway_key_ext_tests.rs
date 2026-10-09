//! 红相契约测试（B001）：`GatewayKeyEntry` 自助 Token 扩展（`ponyllm_config`）。
//!
//! 冻结契约（protocol-agent 锁定，Lead 契约为准，见
//! `.agents/notes/proposed/architecture/2026-10-09-web-user-jwt-and-token-system.md`）：
//! ```rust
//! GatewayKeyEntry += name: Option<String>,          // 1-64 显示名（None = 机器 key）
//!                    model_limits: Option<Vec<String>>, // token 级模型白名单（∩ user.allowed_models）
//!                    quota: Option<u64>,             // token 级用量上限（token 闸）
//!                    user_owned: bool,               // true = 用户自助创建（强制 scope=inference）
//!                    created_by: Option<String>,     // 创建者 user_id
//! ```
//! 全部新字段 `#[serde(default, skip_serializing_if…)]` → 旧 JSON 零迁移。
//!
//! 状态说明：GatewayKeyEntry 字段扩展随契约冻结**实体落地**（非 `todo!()` 桩），
//! 故本文件为契约冻结数据模型测试（预期 PASS，作为 B001 双实体扩展的验收锚）。

use ponyllm_config::{generate_scoped_gateway_key, GatewayKeyEntry, KeyScope};

/// 构造一个含全部自助 Token 新字段的完整 entry。
fn full_token_entry() -> GatewayKeyEntry {
    let (_, mut entry) = generate_scoped_gateway_key("tk-001", KeyScope::Inference);
    entry.name = Some("my-swe-token".to_string());
    entry.model_limits = Some(vec![
        "gpt-4o-mini".to_string(),
        "deepseek/*".to_string(),
    ]);
    entry.quota = Some(1_000_000);
    entry.user_owned = true;
    entry.created_by = Some("usr-007".to_string());
    entry
}

/// GatewayKeyEntry 全字段 serde roundtrip：序列化→反序列化→逐字段相等。
#[test]
fn new_fields_full_serde_roundtrip() {
    // Arrange
    let original = full_token_entry();

    // Act
    let json = serde_json::to_string(&original).expect("GatewayKeyEntry serialize must succeed");
    let restored: GatewayKeyEntry =
        serde_json::from_str(&json).expect("GatewayKeyEntry deserialize must succeed");

    // Assert —— 逐字段还原，避免整结构比较掩盖字段丢失
    assert_eq!(restored.id, original.id);
    assert_eq!(restored.name, Some("my-swe-token".to_string()));
    assert_eq!(restored.model_limits, original.model_limits);
    assert_eq!(restored.quota, Some(1_000_000));
    assert!(restored.user_owned, "user_owned=true must roundtrip");
    assert_eq!(restored.created_by.as_deref(), Some("usr-007"));
    assert_eq!(restored, original);
}

/// 旧 JSON（无新字段）反序列化零迁移：default 生效。
#[test]
fn legacy_json_without_new_fields_deserializes_zero_migration() {
    // Arrange —— 仅含既有字段的旧 GatewayKeyEntry JSON
    let legacy_json = r#"{
        "id": "key-legacy-1",
        "scope": "inference",
        "prefix": "sk-pony-infer-",
        "salt": "abcd",
        "key_hash": "deadbeef",
        "last4": "wxyz"
    }"#;

    // Act
    let entry: GatewayKeyEntry =
        serde_json::from_str(legacy_json).expect("legacy JSON must deserialize");

    // Assert —— 既有字段保留
    assert_eq!(entry.id, "key-legacy-1");
    assert_eq!(entry.scope, KeyScope::Inference);
    assert_eq!(entry.prefix, "sk-pony-infer-");
    assert_eq!(entry.salt, "abcd");
    assert_eq!(entry.key_hash, "deadbeef");
    assert_eq!(entry.last4, "wxyz");
    assert!(!entry.revoked, "revoked must default to false");
    assert_eq!(entry.expires_at, None);
    assert_eq!(entry.user_id, None);
    // Assert —— 新字段默认值生效（零迁移）
    assert_eq!(entry.name, None, "name must default to None");
    assert_eq!(entry.model_limits, None, "model_limits must default to None");
    assert_eq!(entry.quota, None, "quota must default to None");
    assert!(!entry.user_owned, "user_owned must default to false");
    assert_eq!(entry.created_by, None, "created_by must default to None");
}

/// 最简旧 JSON（缺 last4 / revoked / expires_at 等既有可选字段）也应零迁移。
#[test]
fn minimal_legacy_json_deserializes_with_defaults() {
    // Arrange / Act
    let entry: GatewayKeyEntry = serde_json::from_str(
        r#"{"id":"min","scope":"admin","prefix":"sk-pony-admin-","salt":"s","key_hash":"h"}"#,
    )
    .expect("minimal JSON must parse");

    // Assert
    assert_eq!(entry.id, "min");
    assert_eq!(entry.scope, KeyScope::Admin);
    assert_eq!(entry.last4, "****", "legacy last4 must default to ****");
    assert_ne!(entry.revoked, true);
    assert_eq!(entry.user_owned, false);
    assert_eq!(entry.created_by, None);
}

/// skip_serializing_if：None 的新字段 / user_owned=false 不在 JSON 中（存量日志整洁）。
#[test]
fn default_new_fields_are_omitted_from_json() {
    // Arrange —— 仅既有字段的生成 entry（新字段全缺省）
    let (_, entry) = generate_scoped_gateway_key("key-default", KeyScope::Readonly);

    // Act
    let value = serde_json::to_value(&entry).expect("GatewayKeyEntry serialize");

    // Assert
    assert!(value.get("name").is_none(), "None name must be omitted");
    assert!(value.get("model_limits").is_none(), "None model_limits must be omitted");
    assert!(value.get("quota").is_none(), "None quota must be omitted");
    assert!(value.get("created_by").is_none(), "None created_by must be omitted");
    assert!(
        value.get("user_owned").is_none(),
        "user_owned=false must be omitted (is_false skip)"
    );
}

/// user_owned=true 必须出现在序列化结果中，且 roundtrip 保持。
#[test]
fn user_owned_true_is_serialized() {
    // Arrange
    let entry = full_token_entry(); // user_owned = true

    // Act
    let value = serde_json::to_value(&entry).expect("GatewayKeyEntry serialize");

    // Assert
    assert_eq!(value.get("user_owned"), Some(&serde_json::json!(true)));
}

/// KeyScope wire 小写不变（自助 token 强制 scope=inference 依赖该契约）。
#[test]
fn key_scope_serializes_to_lowercase_wire() {
    // Act & Assert
    let entry = full_token_entry(); // scope = inference
    let value = serde_json::to_value(&entry).expect("GatewayKeyEntry serialize");
    assert_eq!(value.get("scope").and_then(|v| v.as_str()), Some("inference"));
}