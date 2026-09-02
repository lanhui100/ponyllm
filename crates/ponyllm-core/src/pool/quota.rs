use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

/// RAII Lease for quota allocation preventing race condition and over-admission in Plan modes
#[derive(Debug)]
pub struct QuotaLease {
    counter: Arc<AtomicI64>,
    committed: bool,
}

impl QuotaLease {
    /// Attempt to atomically pre-acquire a single quota lease
    pub fn try_acquire(counter: &Arc<AtomicI64>) -> Option<Self> {
        let mut curr = counter.load(Ordering::Relaxed);
        loop {
            if curr <= 0 {
                return None;
            }
            match counter.compare_exchange_weak(
                curr,
                curr - 1,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    return Some(Self {
                        counter: counter.clone(),
                        committed: false,
                    })
                }
                Err(actual) => curr = actual,
            }
        }
    }

    /// Commit the lease upon successful upstream dispatch or response
    pub fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for QuotaLease {
    fn drop(&mut self) {
        if !self.committed {
            // Automatically rollback quota if the request was cancelled, dropped, or failed before dispatch
            self.counter.fetch_add(1, Ordering::Relaxed);
        }
    }
}
