use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use pi_domain::contracts::{Clock, IdGen};

pub struct SystemClock;

impl SystemClock {
    pub fn new() -> Arc<SystemClock> {
        Arc::new(SystemClock)
    }
}

impl Clock for SystemClock {
    fn now_unix(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }
}

pub struct UuidGen;

impl UuidGen {
    pub fn new() -> Arc<UuidGen> {
        Arc::new(UuidGen)
    }
}

impl IdGen for UuidGen {
    fn new_id(&self) -> String {
        uuid::Uuid::new_v4().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pi_domain::contracts::{Clock, IdGen};

    #[test]
    fn system_clock_returns_plausible_unix_time() {
        assert!(SystemClock::new().now_unix() > 1_700_000_000);
    }

    #[test]
    fn uuid_gen_returns_unique_ids() {
        let ids = UuidGen::new();
        let (a, b) = (ids.new_id(), ids.new_id());
        assert_ne!(a, b);
        assert_eq!(a.len(), 36);
    }
}
