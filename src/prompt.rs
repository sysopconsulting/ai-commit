use crate::config::Config;

/// Builds the system prompt sent to the LLM.
/// Token-efficient, no few-shot examples.
pub fn build_system_prompt(config: &Config, scope: Option<&str>) -> String {
    let mut lines = vec![
        "You are a git commit message generator. Output ONLY the commit message — no explanation, no preamble, no surrounding text.".to_string(),
        String::new(),
        "Rules:".to_string(),
        "- Format: <type>(<scope>): <subject>".to_string(),
        "- Types: fix, feat, refactor, docs, test, chore, style, perf, build, ci".to_string(),
        "- Subject: imperative, lowercase, no period, max 72 chars".to_string(),
        "- One line unless the changes are complex enough to warrant a body".to_string(),
        "- Output the raw commit message only — do NOT wrap in backticks or markdown".to_string(),
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

/// Conventional commit type prefixes.
const TYPES: &[&str] = &[
    "fix", "feat", "refactor", "docs", "test", "chore", "style", "perf", "build", "ci",
];

/// Clean LLM output: strip preamble, code fences, trailing commentary.
/// Finds the first line that looks like a conventional commit and returns
/// from there, discarding any surrounding prose the model may have added.
pub fn clean_message(raw: &str) -> String {
    let raw = raw.trim();

    // Strip markdown code fences if the whole message is wrapped
    let unwrapped = if raw.starts_with("```") {
        let inner = raw
            .strip_prefix("```")
            .unwrap_or(raw)
            .trim_start_matches(|c: char| c != '\n')
            .trim_start_matches('\n');
        inner
            .strip_suffix("```")
            .unwrap_or(inner)
            .trim()
    } else {
        raw
    };

    let lines: Vec<&str> = unwrapped.lines().collect();

    // Find first line that starts with a conventional commit type
    if let Some(start) = lines.iter().position(|line| is_commit_line(line)) {
        // Take from the commit line onwards
        let mut result: Vec<&str> = Vec::new();
        for &line in &lines[start..] {
            // Stop at trailing commentary (blank line followed by non-commit prose)
            if !result.is_empty() && line.is_empty() {
                // Check if what follows is a body or trailing commentary
                let rest = &lines[start + result.len()..];
                let next_non_empty = rest.iter().find(|l| !l.is_empty());
                if let Some(next) = next_non_empty {
                    // If next non-empty line looks like commentary (starts with
                    // parenthetical, "Note:", "I ", etc.), stop here
                    if is_commentary(next) {
                        break;
                    }
                }
            }
            result.push(line);
        }
        result.join("\n").trim().to_string()
    } else {
        // No conventional commit line found — return trimmed original as fallback
        unwrapped.to_string()
    }
}

fn is_commit_line(line: &str) -> bool {
    let lower = line.trim().to_lowercase();
    TYPES.iter().any(|t| {
        lower.starts_with(&format!("{t}:"))
            || lower.starts_with(&format!("{t}("))
    })
}

fn is_commentary(line: &str) -> bool {
    let trimmed = line.trim();
    let lower = trimmed.to_lowercase();
    // Parenthetical notes about the message itself
    trimmed.starts_with('(')
        // Meta-commentary about the commit message
        || lower.starts_with("note:")
        || lower.starts_with("i used ")
        || lower.starts_with("i chose ")
        || lower.starts_with("here is")
        || lower.starts_with("here's")
        || lower.starts_with("above is")
        || lower.starts_with("let me")
        || lower.starts_with("please ")
        || lower.starts_with("this commit message")
        || lower.starts_with("this message")
        || trimmed == "---"
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
            prompt.contains("commit message"),
            "Should mention 'commit message'"
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

    // --- clean_message tests ---

    #[test]
    fn clean_noop_on_good_message() {
        assert_eq!(
            clean_message("feat(auth): add login endpoint"),
            "feat(auth): add login endpoint",
        );
    }

    #[test]
    fn clean_strips_preamble() {
        let raw = "Here is a concise conventional commit message for the changes:\n\nfeat(auth): add login endpoint";
        assert_eq!(clean_message(raw), "feat(auth): add login endpoint");
    }

    #[test]
    fn clean_strips_preamble_multiline() {
        let raw = "Sure! Based on the diff, here is the commit message:\n\nfix(config): correct default port number\n\nThe default port was wrong.";
        assert_eq!(
            clean_message(raw),
            "fix(config): correct default port number\n\nThe default port was wrong.",
        );
    }

    #[test]
    fn clean_strips_trailing_commentary() {
        let raw = "feat(ui): add dark mode toggle\n\n(Note: I used feat because this is a new feature.)";
        assert_eq!(clean_message(raw), "feat(ui): add dark mode toggle");
    }

    #[test]
    fn clean_strips_code_fences() {
        let raw = "```\nfeat(api): add rate limiting\n```";
        assert_eq!(clean_message(raw), "feat(api): add rate limiting");
    }

    #[test]
    fn clean_preserves_body() {
        let raw = "refactor(db): simplify query builder\n\nRemove redundant join logic and consolidate\ninto a single method.";
        assert_eq!(clean_message(raw), raw);
    }

    #[test]
    fn clean_fallback_if_no_type_found() {
        let raw = "update the readme file";
        assert_eq!(clean_message(raw), raw);
    }

    #[test]
    fn clean_handles_type_without_scope() {
        let raw = "Some explanation.\n\nfix: resolve null pointer on empty input";
        assert_eq!(clean_message(raw), "fix: resolve null pointer on empty input");
    }
}
