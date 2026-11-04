use chrono::DateTime;
use chrono::TimeZone;
use chrono::Utc;
use std::sync::Mutex;

pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

#[derive(Debug)]
pub struct ManualClock {
    current: Mutex<DateTime<Utc>>,
}

impl ManualClock {
    pub fn new(start: DateTime<Utc>) -> Self {
        Self {
            current: Mutex::new(start),
        }
    }
    pub fn at(year: i32, month: u32, day: u32, hour: u32, min: u32, sec: u32) -> Self {
        let start = Utc
            .with_ymd_and_hms(year, month, day, hour, min, sec)
            .single()
            .unwrap_or_else(|| {
                Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0)
                    .single()
                    .expect("fallback time valid")
            });
        Self::new(start)
    }
    pub fn advance(&self, seconds: i64) {
        let mut guard = self.current.lock().expect("manual clock lock usable");
        *guard += chrono::Duration::seconds(seconds);
    }
    pub fn set(&self, value: DateTime<Utc>) {
        let mut guard = self.current.lock().expect("manual clock lock usable");
        *guard = value;
    }
}

impl Clock for ManualClock {
    fn now(&self) -> DateTime<Utc> {
        *self.current.lock().expect("manual clock lock usable")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn manual_clock_is_deterministic() {
        let clock = ManualClock::at(2026, 11, 1, 10, 0, 0);
        let first = clock.now();
        clock.advance(60);
        let second = clock.now();
        assert_eq!((second - first).num_seconds(), 60);
        assert_eq!(first, clock.now() - chrono::Duration::seconds(60));
    }
    #[test]
    fn system_clock_returns_utc() {
        let clock = SystemClock;
        let now = clock.now();
        assert!(now.timestamp() > 0);
    }
}
