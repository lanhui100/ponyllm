//! 红相契约测试（B001）：`UserEntry` / `UserRole` 扩展（`ponyllm_core::user`）。
//!
//! 冻结契约（protocol-agent 锁定，Lead 契约为准）：
//! ```text
//! UserEntry += username: Option<String>, password_hash: Option<String>,
//!              role: UserRole, token_version: u64   // 全带 serde(default, skip_serializing_if…)
//! #[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
//! #[serde(rename_all = "lowercase")]
//! enum UserRole { #[default] User, Admin }
//! ```
//! 契约语义：新字段 serde default → 旧 JSON 反序列化零迁移；role wire 为 "user"/"admin"。
//!
//! GatewayKeyEntry 是 `ponyllm-config` 类型（ponyllm-core 不依赖 config，防成环），按
//! protocol-agent 决策(B) 归 `ponyllm-config/tests/`，不在本文件覆盖（见 Lead 交付清单）。
//!
//! 红相状态：契约尚未实现，断言预期 FAIL。

use ponyllm_core::user::{UserEntry, UserRole};

/// 构造一个含全部登录新字段的完整用户。
fn full_entry() -> UserEntry {
    UserEntry {
        id: "usr-007".to_string(),
        name: "Alice".to_string(),
        enabled: true,
        allowed_models: Some(vec!["gpt-4o-mini".to_string(), "deepseek/*".to_string()]),
        max_tokens: Some(100_000),
        created_at: 1_700_000_000,
        username: Some("alice".to_string()),
        password_hash: Some("$pbkdf2-sha256$i=210000$salt$hash".to_string()),
        role: UserRole::Admin,
        token_version: 9,
    }
}

/// 构造一个全默认/空可选字段的 entry。
fn defaultish_entry() -> UserEntry {
    UserEntry {
        id: "usr-008".to_string(),
        name: String::new(),
        enabled: true,
        allowed_models: None,
        max_tokens: None,
        created_at: 1_700_000_001,
        username: None,
        password_hash: None,
        role: UserRole::User, // 默认角色
        token_version: 0,
    }
}

/// UserEntry 全字段 serde roundtrip：序列化→反序列化→逐字段相等。
#[test]
fn user_entry_full_fields_serde_roundtrip() {
    // Arrange
    let original = full_entry();

    // Act
    let json = serde_json::to_string(&original).expect("UserEntry serialize must succeed");
    let restored: UserEntry =
        serde_json::from_str(&json).expect("UserEntry deserialize must succeed");

    // Assert
    assert_eq!(
        restored, original,
        "full-field roundtrip must reproduce the entry"
    );
}

/// UserRole 序列化为 lowercase 字符串（"user"/"admin"）。
#[test]
fn user_role_serializes_to_lowercase_wire() {
    // Act & Assert
    assert_eq!(
        serde_json::to_value(UserRole::User).expect("UserRole serialize"),
        serde_json::json!("user"),
        "User must serialize to lowercase `user`"
    );
    assert_eq!(
        serde_json::to_value(UserRole::Admin).expect("UserRole serialize"),
        serde_json::json!("admin"),
        "Admin must serialize to lowercase `admin`"
    );
}

/// UserRole 默认值 = User。
#[test]
fn user_role_default_is_user() {
    assert_eq!(UserRole::default(), UserRole::User);
}

/// 旧 JSON（无新字段）反序列化零迁移：default 生效（role=User, username=None,
/// password_hash=None, token_version=0，其余既有默认保持）。
#[test]
fn old_json_without_new_fields_deserializes_zero_migration() {
    // Arrange —— 仅含既有字段的旧 UserEntry JSON
    let old_json = r#"{
        "id": "legacy-1",
        "name": "Legacy User",
        "enabled": true,
        "allowed_models": ["gpt-4o-mini"],
        "max_tokens": 5000,
        "created_at": 1600000000
    }"#;

    // Act
    let entry: UserEntry = serde_json::from_str(old_json).expect("old JSON must deserialize");

    // Assert —— 旧字段保留
    assert_eq!(entry.id, "legacy-1");
    assert_eq!(entry.name, "Legacy User");
    assert!(entry.enabled);
    assert_eq!(entry.allowed_models, Some(vec!["gpt-4o-mini".to_string()]));
    assert_eq!(entry.max_tokens, Some(5000));
    assert_eq!(entry.created_at, 1_600_000_000);
    // Assert —— 新字段默认值生效（零迁移）
    assert_eq!(entry.username, None, "username must default to None");
    assert_eq!(
        entry.password_hash, None,
        "password_hash must default to None"
    );
    assert_eq!(entry.role, UserRole::User, "role must default to User");
    assert_eq!(entry.token_version, 0, "token_version must default to 0");
}

/// 最小旧 JSON（仅 id/name）也应零迁移可解析（enabled/created_at default 兜底）。
#[test]
fn minimal_old_json_deserializes_with_defaults() {
    // Arrange / Act
    let entry: UserEntry =
        serde_json::from_str(r#"{"id":"minimal","name":""}"#).expect("minimal JSON must parse");

    // Assert
    assert_eq!(entry.id, "minimal");
    assert!(
        entry.enabled,
        "enabled defaults to true (default_user_enabled)"
    );
    assert_eq!(entry.role, UserRole::User);
    assert_eq!(entry.token_version, 0);
    assert_eq!(entry.username, None);
}

/// skip_serializing_if：None 的可选新字段不出现在序列化结果中（日志/存储整洁）；
/// 默认值字段（role=User、token_version=0）同样被跳过。
#[test]
fn none_fields_are_omitted_from_json() {
    // Arrange
    let entry = defaultish_entry(); // username/password_hash/allowed_models/max_tokens = None

    // Act
    let value = serde_json::to_value(&entry).expect("UserEntry serialize");

    // Assert —— 可选字段为 None 时应被跳序列化
    assert!(
        value.get("username").is_none(),
        "None username must be omitted"
    );
    assert!(
        value.get("password_hash").is_none(),
        "None password_hash must be omitted"
    );
    assert!(
        value.get("allowed_models").is_none(),
        "None allowed_models must be omitted"
    );
    assert!(
        value.get("max_tokens").is_none(),
        "None max_tokens must be omitted"
    );
    // 默认值角色（User）与默认 tv=0 也被跳过（is_default_user_role / is_zero_token_version）
    assert!(
        value.get("role").is_none(),
        "default role User must be omitted"
    );
    assert!(
        value.get("token_version").is_none(),
        "default token_version 0 must be omitted"
    );
}
