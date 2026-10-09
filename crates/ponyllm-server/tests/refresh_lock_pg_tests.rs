//! Real-PostgreSQL smoke test for [`PostgresRefreshLock`] (P1-qa S2-2 / A5).
//!
//! `#[ignore]` by default — run via `scripts/pg-lock-smoke.sh` against a
//! disposable local PostgreSQL (docker). Requires `PONYLLM_LOCK_DATABASE_URL`;
//! set `PONYLLM_LOCK_SSLMODE=disable` for the smoke (no TLS server cert in the
//! throwaway container). Verifies the server-side mutual exclusion the
//! InMemory double only approximates:
//!   1. two independent gate instances over the SAME advisory lock key:
//!      only one acquires; the other is skipped (different keys block too);
//!   2. after the holder drops its guard, the other acquires;
//!   3. an unreachable lock DB fails closed (Err, never an unlocked refresh).
//!
//! The env-dependent phases run sequentially inside ONE test to avoid the
//! parallel-test env race (`PONYLLM_LOCK_DATABASE_URL` is process-global).

use std::sync::Arc;

use ponyllm_core::pool::refresh_gate::RefreshGate;
use ponyllm_server::refresh_lock::PostgresRefreshLock;

#[tokio::test]
#[ignore = "requires a real PostgreSQL (scripts/pg-lock-smoke.sh)"]
async fn pg_advisory_lock_mutual_exclusion_and_fail_closed() {
    // Two independent gate instances each own a dedicated PG session.
    let a = Arc::new(PostgresRefreshLock::new(None));
    let b = Arc::new(PostgresRefreshLock::new(None));

    let a_guard = a
        .try_acquire("k-1")
        .await
        .expect("gate A connect+acquire")
        .expect("replica A must acquire the global lock first");

    // B (separate connection/session) must be skipped while A holds the lock
    // — even for a different key (global single-lock semantics).
    assert!(
        b.try_acquire("k-2")
            .await
            .expect("gate B query ok")
            .is_none(),
        "replica B must skip while A holds the lock"
    );

    // Drop A's guard: the unlock query runs (spawned), then B acquires.
    drop(a_guard);
    let mut acquired = false;
    for _ in 0..50 {
        if b.try_acquire("k-1")
            .await
            .expect("gate B query ok")
            .is_some()
        {
            acquired = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        acquired,
        "replica B must acquire after A releases the PG advisory lock"
    );

    // Fail-closed leg: point at an unreachable address; the gate must Err
    // with no DSN/credential leak.
    std::env::set_var(
        "PONYLLM_LOCK_DATABASE_URL",
        "host=127.0.0.1 port=1 user=u password=p dbname=d",
    );
    std::env::set_var("PONYLLM_LOCK_SSLMODE", "disable");
    let gate = Arc::new(PostgresRefreshLock::new(None));
    let err = match gate.try_acquire("k-1").await {
        Ok(_) => panic!("unreachable lock DB must fail closed"),
        Err(e) => e,
    };
    assert!(
        !err.to_string().contains("postgres://"),
        "fail-closed error must not leak the DSN: {err}"
    );
    assert!(
        !err.to_string().contains("password"),
        "fail-closed error must not leak credentials: {err}"
    );
}
