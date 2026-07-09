/// `indicatif` template with the animated glyph in the theme accent colour.
/// The colour token goes through `console::Style::from_dotted_str`, which
/// accepts names ("cyan") and numeric 256-colour tokens ("161").
fn spinner_template() -> String {
    match super::theme::theme().accent.template_token() {
        Some(token) => format!("{{spinner:.{token}}} {{msg}}"),
        None => "{spinner} {msg}".to_string(),
    }
}

pub fn spinner(msg: impl Into<String>) -> indicatif::ProgressBar {
    let pb = indicatif::ProgressBar::new_spinner();
    pb.set_style(
        indicatif::ProgressStyle::with_template(&spinner_template())
            .expect("theme spinner template is valid"),
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

    #[test]
    fn spinner_template_embeds_the_theme_accent() {
        let t = spinner_template();
        assert!(t.starts_with("{spinner"), "{t}");
        assert!(t.ends_with("{msg}"), "{t}");
        // The active theme always has a coloured accent, so the template
        // must carry a colour token (named or numeric).
        assert!(t.contains(":."), "{t}");
    }
}
