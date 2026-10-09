use ponyllm_core::executor::UpstreamExecutor;
use ponyllm_core::pool::{ApiKeyEntry, KeyPool, RoutingStrategy};
use std::sync::Arc;

#[tokio::test]
async fn test_pinned_key_executor_selection() {
    let pool = Arc::new(KeyPool::new("antigravity", RoutingStrategy::Priority));
    let k1 = ApiKeyEntry::new("key-1", "token-1", 1, 10);
    let k2 = ApiKeyEntry::new("key-2", "token-2", 2, 10);
    pool.add_key(k1);
    pool.add_key(k2);

    let client = reqwest::Client::new();
    let executor = UpstreamExecutor::with_client(pool.clone(), client, 3);

    assert_eq!(executor.pool.snapshot_keys().len(), 2);
    let pinned_exec = executor.clone().with_pinned_key(Some("key-2".to_string()));
    assert_eq!(pinned_exec.pool.snapshot_keys().len(), 2);
}
