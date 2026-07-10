use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use pi_domain::contracts::LogSink;
use pi_domain::entities::{DeploymentStatus, StageEvent};
use tokio::sync::broadcast;

pub(crate) const HUB_BACKLOG: usize = 1000;

#[derive(Debug, Clone)]
pub enum DeployEvent {
    Line(String),
    Stage(StageEvent),
    Summary(usize),
    Finished(DeploymentStatus),
}

struct StreamState {
    backlog: VecDeque<DeployEvent>,
    tx: broadcast::Sender<DeployEvent>,
}

/// Snapshot of an in-flight deployment's log stream. Finished deployments have
/// no hub entry (subscribe returns None); their logs come from the DB log_tail.
pub struct Subscription {
    pub backlog: Vec<DeployEvent>,
    pub live: broadcast::Receiver<DeployEvent>,
}

pub struct DeployEventsHub {
    streams: Mutex<HashMap<String, StreamState>>,
}

impl DeployEventsHub {
    pub fn new() -> Arc<DeployEventsHub> {
        Arc::new(DeployEventsHub {
            streams: Mutex::new(HashMap::new()),
        })
    }

    pub fn register(self: &Arc<Self>, deployment_id: &str) -> Arc<HubSink> {
        let (tx, _) = broadcast::channel(1024);
        if let Ok(mut streams) = self.streams.lock() {
            streams.insert(
                deployment_id.to_string(),
                StreamState {
                    backlog: VecDeque::new(),
                    tx,
                },
            );
        }
        Arc::new(HubSink {
            hub: Arc::clone(self),
            id: deployment_id.to_string(),
        })
    }

    pub fn subscribe(&self, deployment_id: &str) -> Option<Subscription> {
        let streams = self.streams.lock().ok()?;
        let s = streams.get(deployment_id)?;
        Some(Subscription {
            backlog: s.backlog.iter().cloned().collect(),
            live: s.tx.subscribe(),
        })
    }
}

pub struct HubSink {
    hub: Arc<DeployEventsHub>,
    id: String,
}

impl HubSink {
    fn push(&self, ev: DeployEvent) {
        if let Ok(mut streams) = self.hub.streams.lock() {
            if let Some(s) = streams.get_mut(&self.id) {
                if s.backlog.len() == HUB_BACKLOG {
                    s.backlog.pop_front();
                }
                s.backlog.push_back(ev.clone());
                let _ = s.tx.send(ev);
            }
        }
    }
}

impl LogSink for HubSink {
    fn line(&self, line: &str) {
        self.push(DeployEvent::Line(line.to_string()));
    }

    fn stage(&self, ev: &StageEvent) {
        self.push(DeployEvent::Stage(ev.clone()));
    }

    fn summary(&self, services: usize) {
        self.push(DeployEvent::Summary(services));
    }

    fn finished(&self, status: DeploymentStatus) {
        // Remove the entry so its backlog (up to HUB_BACKLOG events) is freed.
        // Active live subscribers already hold a Receiver; they get the
        // Finished event from the buffered channel before it closes.
        // New subscribers after this point fall back to the DB log_tail path.
        if let Ok(mut streams) = self.hub.streams.lock() {
            if let Some(s) = streams.remove(&self.id) {
                let _ = s.tx.send(DeployEvent::Finished(status));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn subscriber_gets_backlog_then_live_events() {
        let hub = DeployEventsHub::new();
        let sink = hub.register("d1");
        sink.line("early-1");
        sink.line("early-2");

        let mut sub = hub.subscribe("d1").unwrap();
        assert!(matches!(&sub.backlog[0], DeployEvent::Line(l) if l == "early-1"));
        assert!(matches!(&sub.backlog[1], DeployEvent::Line(l) if l == "early-2"));

        sink.line("live-1");
        sink.finished(DeploymentStatus::Success);
        assert!(matches!(sub.live.recv().await.unwrap(), DeployEvent::Line(l) if l == "live-1"));
        assert!(matches!(
            sub.live.recv().await.unwrap(),
            DeployEvent::Finished(DeploymentStatus::Success)
        ));
    }

    #[tokio::test]
    async fn subscribe_after_finished_returns_none() {
        // finished() removes the entry so memory is freed. New subscribers
        // after completion use the DB log_tail fallback path in the HTTP handler.
        let hub = DeployEventsHub::new();
        let sink = hub.register("d1");
        sink.line("a");
        sink.finished(DeploymentStatus::Failed);

        assert!(hub.subscribe("d1").is_none());
    }

    #[tokio::test]
    async fn unknown_deployment_returns_none() {
        assert!(DeployEventsHub::new().subscribe("nope").is_none());
    }

    #[tokio::test]
    async fn backlog_is_capped() {
        let hub = DeployEventsHub::new();
        let sink = hub.register("d1");
        for i in 0..(HUB_BACKLOG + 5) {
            sink.line(&format!("line-{i}"));
        }
        let sub = hub.subscribe("d1").unwrap();
        assert_eq!(sub.backlog.len(), HUB_BACKLOG);
        assert!(matches!(&sub.backlog[0], DeployEvent::Line(l) if l == "line-5"));
    }

    #[tokio::test]
    async fn backlog_replays_lines_stages_and_summary_in_order() {
        use pi_domain::entities::{StageEvent, StageStatus};
        let hub = DeployEventsHub::new();
        let sink = hub.register("d1");
        sink.stage(&StageEvent::started("fetch"));
        sink.line("cloning");
        sink.stage(&StageEvent::ok(
            "fetch",
            std::time::Duration::from_millis(2100),
        ));
        sink.summary(2);

        let sub = hub.subscribe("d1").unwrap();
        match &sub.backlog[..] {
            [DeployEvent::Stage(s0), DeployEvent::Line(l), DeployEvent::Stage(s1), DeployEvent::Summary(n)] =>
            {
                assert_eq!(s0.status, StageStatus::Started);
                assert_eq!(l, "cloning");
                assert_eq!(s1.elapsed_ms, Some(2100));
                assert_eq!(*n, 2);
            }
            other => panic!("unexpected backlog: {other:?}"),
        }
    }
}
