use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pi_domain::contracts::{HostMetricsStore, TempProbe};
use pi_domain::entities::HostSample;
use sysinfo::System;

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

/// Owns the ring buffer behind a std `Mutex` (critical section is a
/// push/evict with no `.await` held) plus the loop's sysinfo + temp state.
pub struct HostMetricsSampler {
    buf: Arc<Mutex<RingBuffer>>,
    system: System,
    temp: Arc<dyn TempProbe>,
    interval: Duration,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn take_sample(system: &mut System, temp: &Arc<dyn TempProbe>) -> HostSample {
    system.refresh_cpu_usage();
    system.refresh_memory();
    HostSample {
        at_ms: now_ms(),
        cpu_percent: f64::from(system.global_cpu_usage()),
        mem_used_bytes: system.used_memory(),
        mem_total_bytes: system.total_memory(),
        temp_celsius: temp.cpu_celsius(),
    }
}

impl HostMetricsSampler {
    pub fn new(
        mut system: System,
        temp: Arc<dyn TempProbe>,
        window: Duration,
        interval: Duration,
    ) -> HostMetricsSampler {
        let mut ring = RingBuffer::new(window);
        // Immediate seed so `latest()` is Some before the first request.
        ring.push(take_sample(&mut system, &temp));
        HostMetricsSampler {
            buf: Arc::new(Mutex::new(ring)),
            system,
            temp,
            interval,
        }
    }

    pub fn handle(&self) -> Arc<dyn HostMetricsStore> {
        Arc::new(HostMetricsHandle {
            buf: Arc::clone(&self.buf),
        })
    }

    pub fn start(self) {
        let HostMetricsSampler {
            buf,
            mut system,
            temp,
            interval,
        } = self;
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.tick().await; // first tick fires immediately — skip (already seeded)
            loop {
                ticker.tick().await;
                let sample = take_sample(&mut system, &temp);
                if let Ok(mut ring) = buf.lock() {
                    ring.push(sample);
                }
            }
        });
    }
}

/// Cloneable read handle over the shared ring buffer.
struct HostMetricsHandle {
    buf: Arc<Mutex<RingBuffer>>,
}

impl HostMetricsStore for HostMetricsHandle {
    fn latest(&self) -> Option<HostSample> {
        self.buf.lock().ok().and_then(|r| r.latest())
    }

    fn history(&self) -> Vec<HostSample> {
        self.buf.lock().map(|r| r.snapshot()).unwrap_or_default()
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

#[cfg(test)]
mod sampler_tests {
    use super::*;
    use std::sync::Arc;
    use sysinfo::System;

    struct FixedTemp(Option<f64>);
    impl pi_domain::contracts::TempProbe for FixedTemp {
        fn cpu_celsius(&self) -> Option<f64> {
            self.0
        }
    }

    #[test]
    fn construction_takes_one_immediate_sample() {
        let sampler = HostMetricsSampler::new(
            System::new(),
            Arc::new(FixedTemp(Some(41.0))),
            Duration::from_secs(300),
            Duration::from_secs(2),
        );
        let handle = sampler.handle();
        let latest = handle.latest().expect("pre-seeded sample present");
        assert!(latest.mem_total_bytes > 0, "sysinfo memory read");
        assert_eq!(latest.temp_celsius, Some(41.0));
        assert_eq!(handle.history().len(), 1);
    }

    #[tokio::test]
    async fn started_loop_appends_more_samples() {
        let sampler = HostMetricsSampler::new(
            System::new(),
            Arc::new(FixedTemp(None)),
            Duration::from_secs(300),
            Duration::from_millis(20),
        );
        let handle = sampler.handle();
        sampler.start();
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert!(handle.history().len() >= 2, "sampler appended samples");
    }
}
