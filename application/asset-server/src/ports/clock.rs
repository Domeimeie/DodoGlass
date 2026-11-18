//! The current time, so that tests can move it forward instead of waiting.

use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub trait Clock: Send + Sync + 'static {
    /// Milliseconds since the Unix epoch.
    fn now_ms(&self) -> i64;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }
}

/// A clock that only moves when told to. Starts at 2026-11-17T00:00:00Z.
pub struct FakeClock(Mutex<i64>);

impl Default for FakeClock {
    fn default() -> Self {
        FakeClock(Mutex::new(1_794_873_600_000))
    }
}

impl FakeClock {
    pub fn advance(&self, by: Duration) {
        *self.0.lock().unwrap() += by.as_millis() as i64;
    }
}

impl Clock for FakeClock {
    fn now_ms(&self) -> i64 {
        *self.0.lock().unwrap()
    }
}
