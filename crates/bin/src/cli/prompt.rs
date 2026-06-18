use anyhow::Result;

/// Abstracts interactive prompts so wizard logic is testable without a TTY.
pub trait Prompter {
    fn text(&mut self, label: &str, default: Option<&str>) -> Result<String>;
    fn confirm(&mut self, label: &str, default: bool) -> Result<bool>;
    fn select(&mut self, label: &str, options: &[String], default: usize) -> Result<String>;
}

pub struct InquirePrompter;

/// Convert inquire interruptions/cancellations into a clean one-line error
/// instead of a raw error trace (PR #7 S3).
fn map_inquire(err: inquire::InquireError) -> anyhow::Error {
    use inquire::InquireError::*;
    match err {
        OperationInterrupted | OperationCanceled => anyhow::anyhow!("interrupted by user"),
        other => anyhow::Error::new(other),
    }
}

impl Prompter for InquirePrompter {
    fn text(&mut self, label: &str, default: Option<&str>) -> Result<String> {
        let mut q = inquire::Text::new(label);
        if let Some(d) = default {
            q = q.with_default(d);
        }
        q.prompt().map_err(map_inquire)
    }
    fn confirm(&mut self, label: &str, default: bool) -> Result<bool> {
        inquire::Confirm::new(label).with_default(default).prompt().map_err(map_inquire)
    }
    fn select(&mut self, label: &str, options: &[String], default: usize) -> Result<String> {
        Ok(inquire::Select::new(label, options.to_vec())
            .with_starting_cursor(default)
            .prompt()
            .map_err(map_inquire)?)
    }
}

#[cfg(test)]
pub struct ScriptedPrompter {
    pub texts: std::collections::VecDeque<String>,
    pub confirms: std::collections::VecDeque<bool>,
    pub selects: std::collections::VecDeque<String>,
}

#[cfg(test)]
impl Prompter for ScriptedPrompter {
    fn text(&mut self, _label: &str, default: Option<&str>) -> Result<String> {
        Ok(self.texts.pop_front().unwrap_or_else(|| default.unwrap_or("").to_string()))
    }
    fn confirm(&mut self, _label: &str, default: bool) -> Result<bool> {
        Ok(self.confirms.pop_front().unwrap_or(default))
    }
    fn select(&mut self, _label: &str, options: &[String], default: usize) -> Result<String> {
        Ok(self.selects.pop_front().unwrap_or_else(|| options[default].clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripted_prompter_returns_defaults_when_empty() {
        let mut p = ScriptedPrompter {
            texts: Default::default(),
            confirms: Default::default(),
            selects: Default::default(),
        };
        assert_eq!(p.text("name", Some("rateme")).unwrap(), "rateme");
        assert!(!p.confirm("ok?", false).unwrap());
        assert_eq!(p.select("x", &["a".into(), "b".into()], 1).unwrap(), "b");
    }
}
