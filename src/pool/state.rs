//! Shared worker-pool counters and event bookkeeping.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::RateLimitEvent;

use super::PoolStats;

#[derive(Default, Debug)]
pub(super) struct SharedPoolState {
    pub(super) completed: AtomicU64,
    pub(super) failures: AtomicU64,
    pub(super) worker_recycles: AtomicU64,
    pub(super) rate_limit_events: Mutex<Vec<RateLimitEvent>>,
    pub(super) fatal_quota: AtomicBool,
    pub(super) alive_mask: AtomicU64,
}

impl SharedPoolState {
    pub(super) fn stats(&self) -> PoolStats {
        PoolStats {
            completed: self.completed.load(Ordering::Acquire),
            failures: self.failures.load(Ordering::Acquire),
            rate_limit_events: self
                .rate_limit_events
                .lock()
                .map(|events| events.clone())
                .unwrap_or_default(),
            worker_recycles: self.worker_recycles.load(Ordering::Acquire),
        }
    }

    pub(super) fn push_rate_limit_event(&self, event: RateLimitEvent) {
        if let Ok(mut events) = self.rate_limit_events.lock() {
            events.push(event);
        }
    }
}
