use crate::config::Config;

/// Builds the system prompt sent to the LLM.
/// Token-efficient, no few-shot examples.
pub fn build_system_prompt(config: &Config, scope: Option<&str>) -> String {
    let mut lines = vec![
        "You are a git commit message generator. Write a concise conventional commit message for the following changes.".to_string(),
        String::new(),
        "Rules:".to_string(),
        "- Format: <type>(<scope>): <subject>".to_string(),
        "- Types: fix, feat, refactor, docs, test, chore, style, perf, build, ci".to_string(),
        "- Subject: imperative, lowercase, no period, max 72 chars".to_string(),
        "- One line unless the changes are complex enough to warrant a body".to_string(),
    ];

    if config.one_line {
        lines.push("- Output only a single-line commit message, no body".to_string());
    }

    if config.emoji {
        lines.push("- Prefix the subject with a relevant emoji".to_string());
    }

    if config.language != "en" {
        lines.push(format!("- Write the message in language: {}", config.language));
    }

    if let Some(s) = scope {
        lines.push(format!(
            "- Detected scope: {} — use it unless the changes clearly warrant a different scope",
            s
        ));
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    // 1. Default prompt contains core rule keywords
    #[test]
    fn default_prompt_contains_rules() {
        let config = Config::default();
        let prompt = build_system_prompt(&config, None);
        assert!(
            prompt.contains("conventional commit"),
            "Should mention 'conventional commit'"
        );
        assert!(
            prompt.contains("imperative"),
            "Should mention 'imperative'"
        );
        assert!(
            prompt.contains("max 72 chars"),
            "Should mention 'max 72 chars'"
        );
    }

    // 2. Scope hint is included when scope is provided
    #[test]
    fn prompt_includes_scope_hint() {
        let config = Config::default();
        let prompt = build_system_prompt(&config, Some("auth"));
        assert!(
            prompt.contains("auth"),
            "Prompt should contain the detected scope 'auth'"
        );
    }

    // 3. No scope instruction line when scope is None
    #[test]
    fn prompt_omits_scope_when_none() {
        let config = Config::default();
        let prompt = build_system_prompt(&config, None);
        assert!(
            !prompt.contains("Detected scope:"),
            "Prompt should not contain 'Detected scope:' when scope is None"
        );
    }

    // 4. Emoji instruction present when emoji=true
    #[test]
    fn prompt_includes_emoji_instruction() {
        let mut config = Config::default();
        config.emoji = true;
        let prompt = build_system_prompt(&config, None);
        assert!(
            prompt.contains("emoji"),
            "Prompt should contain 'emoji' when emoji is enabled"
        );
    }

    // 5. No emoji mention when emoji=false
    #[test]
    fn prompt_omits_emoji_when_disabled() {
        let mut config = Config::default();
        config.emoji = false;
        let prompt = build_system_prompt(&config, None);
        assert!(
            !prompt.contains("emoji"),
            "Prompt should not contain 'emoji' when emoji is disabled"
        );
    }

    // 6. Language instruction included when language is not English
    #[test]
    fn prompt_includes_language_when_not_english() {
        let mut config = Config::default();
        config.language = "ja".to_string();
        let prompt = build_system_prompt(&config, None);
        assert!(
            prompt.contains("ja"),
            "Prompt should contain the language code 'ja'"
        );
    }

    #[test]
    fn prompt_includes_one_line_instruction() {
        let mut config = Config::default();
        config.one_line = true;
        let prompt = build_system_prompt(&config, None);
        assert!(prompt.contains("single-line"), "should mention single-line: {prompt}");
    }

    // 7. Basic sanity: prompt is not empty with default config
    #[test]
    fn prompt_includes_user_context() {
        let config = Config::default();
        let prompt = build_system_prompt(&config, None);
        assert!(!prompt.is_empty(), "Prompt should not be empty");
    }
}
