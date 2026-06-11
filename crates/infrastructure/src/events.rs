use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use pi_domain::contracts::LogSink;
use pi_domain::entities::DeploymentStatus;
use tokio::sync::broadcast;

pub(crate) const HUB_BACKLOG: usize = 1000;

#[derive(Debug, Clone)]
pub enum DeployEvent {
    Line(String),
    Finished(DeploymentStatus),
}

struct StreamState {
    lines: VecDeque<String>,
    finished: Option<DeploymentStatus>,
    tx: broadcast::Sender<DeployEvent>,
}

pub struct Subscription {
    pub backlog: Vec<String>,
    pub finished: Option<DeploymentStatus>,
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
                    lines: VecDeque::new(),
                    finished: None,
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
            backlog: s.lines.iter().cloned().collect(),
            finished: s.finished,
            live: s.tx.subscribe(),
        })
    }
}

pub struct HubSink {
    hub: Arc<DeployEventsHub>,
    id: String,
}

impl LogSink for HubSink {
    fn line(&self, line: &str) {
        if let Ok(mut streams) = self.hub.streams.lock() {
            if let Some(s) = streams.get_mut(&self.id) {
                if s.lines.len() == HUB_BACKLOG {
                    s.lines.pop_front();
                }
                s.lines.push_back(line.to_string());
                let _ = s.tx.send(DeployEvent::Line(line.to_string()));
            }
        }
    }

    fn finished(&self, status: DeploymentStatus) {
        if let Ok(mut streams) = self.hub.streams.lock() {
            if let Some(s) = streams.get_mut(&self.id) {
                s.finished = Some(status);
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
        assert_eq!(
            sub.backlog,
            vec!["early-1".to_string(), "early-2".to_string()]
        );
        assert!(sub.finished.is_none());

        sink.line("live-1");
        sink.finished(DeploymentStatus::Success);
        assert!(matches!(sub.live.recv().await.unwrap(), DeployEvent::Line(l) if l == "live-1"));
        assert!(matches!(
            sub.live.recv().await.unwrap(),
            DeployEvent::Finished(DeploymentStatus::Success)
        ));
    }

    #[tokio::test]
    async fn late_subscriber_sees_finished_in_snapshot() {
        let hub = DeployEventsHub::new();
        let sink = hub.register("d1");
        sink.line("a");
        sink.finished(DeploymentStatus::Failed);

        let sub = hub.subscribe("d1").unwrap();
        assert_eq!(sub.backlog, vec!["a".to_string()]);
        assert_eq!(sub.finished, Some(DeploymentStatus::Failed));
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
        assert_eq!(sub.backlog.first().unwrap(), "line-5");
    }
}
