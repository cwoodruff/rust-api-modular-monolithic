//! The current time, ported from `TimeProvider`.
//!
//! The outbox, the dead-letter retry and the integrity job all ask a clock
//! rather than calling `Utc::now()`, so a test can hold time still and move it
//! by exactly the retry delay it wants to observe — what upstream's tests do
//! with a manual `TimeProvider`.

use std::sync::{Arc, Mutex};

use chrono::{DateTime, TimeDelta, Utc};

/// Where the time comes from.
pub trait Clock: Send + Sync + std::fmt::Debug {
    /// Now, in UTC.
    fn now(&self) -> DateTime<Utc>;
}

/// The wall clock. Port of `TimeProvider.System`.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// A clock that moves only when told to.
#[derive(Debug, Clone)]
pub struct ManualClock {
    now: Arc<Mutex<DateTime<Utc>>>,
}

impl ManualClock {
    /// A clock stopped at `start`.
    #[must_use]
    pub fn new(start: DateTime<Utc>) -> Self {
        Self {
            now: Arc::new(Mutex::new(start)),
        }
    }

    /// Moves the clock forward.
    pub fn advance(&self, by: TimeDelta) {
        let mut now = self
            .now
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *now += by;
    }
}

impl Clock for ManualClock {
    fn now(&self) -> DateTime<Utc> {
        *self
            .now
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// The clock every module shares unless a test supplies its own.
#[must_use]
pub fn system() -> Arc<dyn Clock> {
    Arc::new(SystemClock)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_manual_clock_moves_only_when_advanced() {
        let start = Utc::now();
        let clock = ManualClock::new(start);

        assert_eq!(clock.now(), start);
        clock.advance(TimeDelta::seconds(5));
        assert_eq!(clock.now(), start + TimeDelta::seconds(5));
    }
}
