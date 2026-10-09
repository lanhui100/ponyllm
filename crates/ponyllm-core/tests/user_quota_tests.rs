use ponyllm_core::user::{UserCheckError, UserEntry, UserQuotaTracker, UserRole};

#[test]
fn test_user_quota_tracker_model_filtering() {
    // Arrange
    let tracker = UserQuotaTracker::new();
    let user = UserEntry {
        id: "u1".to_string(),
        name: "Restricted User".to_string(),
        enabled: true,
        allowed_models: Some(vec!["gpt-4o-mini".to_string(), "deepseek/*".to_string()]),
        max_tokens: Some(10_000),
        created_at: 1000,
        username: None,
        password_hash: None,
        role: UserRole::User,
        token_version: 0,
    };
    tracker.upsert_user(user);

    // Act & Assert - Allowed exact
    assert_eq!(tracker.check_access("u1", "gpt-4o-mini"), Ok(()));
    // Act & Assert - Allowed pattern
    assert_eq!(tracker.check_access("u1", "deepseek/deepseek-chat"), Ok(()));
    // Act & Assert - Forbidden model
    assert_eq!(
        tracker.check_access("u1", "claude-3-5-sonnet"),
        Err(UserCheckError::ModelNotAllowed {
            user_id: "u1".to_string(),
            model: "claude-3-5-sonnet".to_string(),
        })
    );
}

#[test]
fn test_user_quota_tracker_exhaustion() {
    // Arrange
    let tracker = UserQuotaTracker::new();
    let user = UserEntry {
        id: "u2".to_string(),
        name: "Budget User".to_string(),
        enabled: true,
        allowed_models: None, // All models allowed
        max_tokens: Some(100),
        created_at: 1000,
        username: None,
        password_hash: None,
        role: UserRole::User,
        token_version: 0,
    };
    tracker.upsert_user(user);

    // Initial check
    assert_eq!(tracker.check_access("u2", "any-model"), Ok(()));

    // Consume 60 tokens
    tracker.record_tokens("u2", 60);
    assert_eq!(tracker.get_used_tokens("u2"), 60);
    assert_eq!(tracker.check_access("u2", "any-model"), Ok(()));

    // Consume 50 tokens (total 110 >= 100)
    tracker.record_tokens("u2", 50);
    assert_eq!(tracker.get_used_tokens("u2"), 110);

    // Act & Assert - Expect QuotaExhausted
    assert_eq!(
        tracker.check_access("u2", "any-model"),
        Err(UserCheckError::QuotaExhausted {
            user_id: "u2".to_string(),
            used_tokens: 110,
            max_tokens: 100,
        })
    );

    // Reset usage
    tracker.reset_usage("u2");
    assert_eq!(tracker.get_used_tokens("u2"), 0);
    assert_eq!(tracker.check_access("u2", "any-model"), Ok(()));
}

#[test]
fn test_user_quota_tracker_disabled_user() {
    // Arrange
    let tracker = UserQuotaTracker::new();
    let user = UserEntry {
        id: "u3".to_string(),
        name: "Disabled User".to_string(),
        enabled: false,
        allowed_models: None,
        max_tokens: None,
        created_at: 1000,
        username: None,
        password_hash: None,
        role: UserRole::User,
        token_version: 0,
    };
    tracker.upsert_user(user);

    // Act & Assert
    assert_eq!(
        tracker.check_access("u3", "any-model"),
        Err(UserCheckError::UserDisabled {
            user_id: "u3".to_string(),
        })
    );
}
