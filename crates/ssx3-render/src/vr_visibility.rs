//! Head-gaze generation shared by the native game and frame publisher.
//! It only tags visual frames; it does not change simulation state.

use std::sync::atomic::{AtomicU64, Ordering};

static EPOCH: AtomicU64 = AtomicU64::new(0);

pub fn set_epoch(epoch: u64) {
    EPOCH.store(epoch, Ordering::Release);
}

pub fn epoch() -> u64 {
    EPOCH.load(Ordering::Acquire)
}
