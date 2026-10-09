use ponyllm_config::{UserEntry, UserRole};

#[test]
fn test_user_entry_serialization_roundtrip() {
    let user = UserEntry {
        id: "usr_alice".to_string(),
        name: "Alice Developer".to_string(),
        enabled: true,
        allowed_models: Some(vec!["gpt-4o-mini".to_string(), "claude-*".to_string()]),
        max_tokens: Some(50_000),
        created_at: 1720000000,
        username: None,
        password_hash: None,
        role: UserRole::User,
        token_version: 0,
    };

    let serialized = serde_json::to_string(&user).expect("serialize user");
    let deserialized: UserEntry = serde_json::from_str(&serialized).expect("deserialize user");
    assert_eq!(user, deserialized);
}

#[test]
fn test_gateway_key_with_user_id() {
    let (plain, mut entry) =
        ponyllm_config::generate_scoped_gateway_key("key-1", ponyllm_config::KeyScope::Inference);
    assert!(plain.starts_with("sk-pony-infer-"));
    assert_eq!(entry.user_id, None);

    entry.user_id = Some("usr_alice".to_string());
    let serialized = serde_json::to_string(&entry).expect("serialize key");
    let deserialized: ponyllm_config::GatewayKeyEntry =
        serde_json::from_str(&serialized).expect("deserialize key");
    assert_eq!(deserialized.user_id.as_deref(), Some("usr_alice"));
}
