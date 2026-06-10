#[derive(Debug, PartialEq, Eq)]
pub struct SseEvent {
    pub event: String,
    pub data: String,
}

#[derive(Default)]
pub struct SseParser {
    buf: String,
}

impl SseParser {
    pub fn push(&mut self, chunk: &str) -> Vec<SseEvent> {
        self.buf.push_str(&chunk.replace('\r', ""));
        let mut events = Vec::new();
        while let Some(pos) = self.buf.find("\n\n") {
            let record: String = self.buf.drain(..pos + 2).collect();
            let mut event = String::from("message");
            let mut data_lines: Vec<String> = Vec::new();
            for line in record.lines() {
                if let Some(v) = line.strip_prefix("event:") {
                    event = v.trim().to_string();
                } else if let Some(v) = line.strip_prefix("data:") {
                    data_lines.push(v.strip_prefix(' ').unwrap_or(v).to_string());
                }
            }
            if !data_lines.is_empty() || event != "message" {
                events.push(SseEvent { event, data: data_lines.join("\n") });
            }
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_event_split_across_chunks() {
        let mut p = SseParser::default();
        assert!(p.push("event: log\nda").is_empty());
        let events = p.push("ta: hello\n\nevent: finished\ndata: success\n\n");
        assert_eq!(
            events,
            vec![
                SseEvent { event: "log".into(), data: "hello".into() },
                SseEvent { event: "finished".into(), data: "success".into() },
            ]
        );
    }

    #[test]
    fn ignores_keepalive_comments_and_handles_crlf() {
        let mut p = SseParser::default();
        assert!(p.push(":\r\n\r\n").is_empty());
        let events = p.push("event: log\r\ndata: line\r\n\r\n");
        assert_eq!(events, vec![SseEvent { event: "log".into(), data: "line".into() }]);
    }
}
