use std::collections::VecDeque;
use std::time::Duration;

use pi_domain::entities::HostSample;

/// Age-evicted ring buffer of host samples. Pure — no clock, no IO, no tokio.
/// Eviction is relative to the newest sample's timestamp so tests are
/// deterministic.
pub struct RingBuffer {
    samples: VecDeque<HostSample>,
    window_ms: i64,
}

impl RingBuffer {
    pub fn new(window: Duration) -> RingBuffer {
        RingBuffer {
            samples: VecDeque::new(),
            window_ms: window.as_millis() as i64,
        }
    }

    pub fn push(&mut self, sample: HostSample) {
        let cutoff = sample.at_ms - self.window_ms;
        self.samples.push_back(sample);
        while let Some(front) = self.samples.front() {
            if front.at_ms < cutoff {
                self.samples.pop_front();
            } else {
                break;
            }
        }
    }

    pub fn latest(&self) -> Option<HostSample> {
        self.samples.back().cloned()
    }

    pub fn snapshot(&self) -> Vec<HostSample> {
        self.samples.iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(at_ms: i64) -> HostSample {
        HostSample {
            at_ms,
            cpu_percent: 1.0,
            mem_used_bytes: 1,
            mem_total_bytes: 2,
            temp_celsius: None,
        }
    }

    #[test]
    fn evicts_samples_older_than_the_window() {
        let mut rb = RingBuffer::new(Duration::from_secs(300)); // 300_000 ms
        rb.push(sample(0));
        rb.push(sample(100_000));
        rb.push(sample(200_000));
        rb.push(sample(400_000)); // cutoff = 100_000 → drops at_ms 0
        let snap = rb.snapshot();
        assert_eq!(
            snap.iter().map(|s| s.at_ms).collect::<Vec<_>>(),
            vec![100_000, 200_000, 400_000]
        );
    }

    #[test]
    fn latest_returns_the_newest_sample() {
        let mut rb = RingBuffer::new(Duration::from_secs(300));
        assert_eq!(rb.latest(), None);
        rb.push(sample(1));
        rb.push(sample(2));
        assert_eq!(rb.latest().unwrap().at_ms, 2);
    }

    #[test]
    fn snapshot_is_oldest_to_newest() {
        let mut rb = RingBuffer::new(Duration::from_secs(300));
        rb.push(sample(10));
        rb.push(sample(20));
        rb.push(sample(30));
        assert_eq!(
            rb.snapshot().iter().map(|s| s.at_ms).collect::<Vec<_>>(),
            vec![10, 20, 30]
        );
    }
}
