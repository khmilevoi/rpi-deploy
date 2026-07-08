pub fn spinner(msg: impl Into<String>) -> indicatif::ProgressBar {
    let pb = indicatif::ProgressBar::new_spinner();
    pb.set_style(
        indicatif::ProgressStyle::with_template("{spinner} {msg}")
            .expect("static spinner template is valid"),
    );
    pb.set_message(msg.into());
    pb.enable_steady_tick(std::time::Duration::from_millis(100));
    pb
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spinner_starts_unfinished_and_can_be_finished() {
        let pb = spinner("connecting...");
        assert!(!pb.is_finished());
        pb.finish_and_clear();
        assert!(pb.is_finished());
    }
}
