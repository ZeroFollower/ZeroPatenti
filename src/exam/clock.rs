use std::time::Instant;

/// Elapsed foreground time since a persisted checkpoint or resume.
pub struct ActiveClock {
    base_ms: i64,
    started: Instant,
}

impl ActiveClock {
    pub fn new(base_ms: i64) -> Self {
        Self {
            base_ms,
            started: Instant::now(),
        }
    }

    pub fn active_ms(&self) -> i64 {
        self.at(Instant::now())
    }

    fn at(&self, instant: Instant) -> i64 {
        let elapsed_ms = instant.saturating_duration_since(self.started).as_millis();
        self.base_ms
            .saturating_add(i64::try_from(elapsed_ms).unwrap_or(i64::MAX))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn adds_only_time_since_new_foreground_segment() {
        let clock = ActiveClock::new(753);
        assert_eq!(clock.at(clock.started + Duration::from_millis(247)), 1000);
        assert_eq!(clock.at(clock.started), 753);
    }
}
