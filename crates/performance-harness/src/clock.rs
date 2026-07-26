//! Monotonic timing inputs for bounded evidence collection.

use std::time::Instant;

pub trait MonotonicClock: std::fmt::Debug {
    fn now_ns(&self) -> u64;
}

#[derive(Debug)]
pub struct SystemMonotonicClock {
    origin: Instant,
}

impl SystemMonotonicClock {
    pub fn start() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl Default for SystemMonotonicClock {
    fn default() -> Self {
        Self::start()
    }
}

impl MonotonicClock for SystemMonotonicClock {
    fn now_ns(&self) -> u64 {
        u64::try_from(self.origin.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }
}
