use crate::config::Config;

/// Builds the system prompt sent to the LLM.
/// Token-efficient, no few-shot examples.
pub fn build_system_prompt(config: &Config, scope: Option<&str>) -> String {
    let format_rule = if scope.is_some() {
        "- Format: <type>(<scope>): <subject>"
    } else {
        "- Format: <type>: <subject>"
    };
    let mut lines = vec![
        "You are a git commit message generator. Output ONLY the raw commit message — no explanation, no preamble, no markdown fences.".to_string(),
        String::new(),
        "Rules:".to_string(),
        format_rule.to_string(),
        "- Types: fix, feat, refactor, docs, test, chore, style, perf, build, ci".to_string(),
        "- Pick the type of the primary change — the reason this commit exists; feat or fix outweighs accompanying refactor/test/docs/chore work".to_string(),
        "- Subject: imperative, lowercase, no period, max 72 chars, naming the primary change concretely".to_string(),
        "- Never write vague subjects like \"update code\", \"improve handling\", \"various changes\"".to_string(),
        "- Describe only what the diff and file list show — never invent changes".to_string(),
    ];

    if config.one_line {
        lines.push("- Output only a single-line commit message, no body".to_string());
    } else {
        lines.push(
            "- Body: when the commit contains several distinct changes, add a blank line then 2-6 \"- \" bullets, one per logical change, most important first, each under 72 chars, naming modules and behavior rather than filenames"
                .to_string(),
        );
        lines.push("- Omit the body when it would only restate the subject".to_string());
    }

    lines.push(
        "- Input may start with a \"Files changed\" list ranked by relevance; hunks may be truncated or omitted — use the list to cover all significant changes"
            .to_string(),
    );
    lines.push(
        "- Files marked (low-signal) are lockfiles or generated output; mention them only if nothing else changed"
            .to_string(),
    );

    if config.emoji {
        lines.push("- Prefix the subject with a relevant emoji".to_string());
    }

    if config.language != "en" {
        lines.push(format!(
            "- Write the message in language: {}",
            config.language
        ));
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

/// If `s` begins with a `<think>`/`<thinking>` open or close tag
/// (case-insensitive, attributes tolerated), return `(tag_len, is_closing)`.
/// Shared by [`clean_message`] and the streaming display filter so both use
/// the identical tag grammar.
pub(crate) fn think_tag_at(s: &str) -> Option<(usize, bool)> {
    let bytes = s.as_bytes();
    if bytes.first() != Some(&b'<') {
        return None;
    }
    let mut i = 1;
    let closing = bytes.get(i) == Some(&b'/');
    if closing {
        i += 1;
    }
    const FULL: &[u8] = b"thinking";
    const SHORT: &[u8] = b"think";
    let name_len = if bytes.len() >= i + FULL.len()
        && bytes[i..i + FULL.len()].eq_ignore_ascii_case(FULL)
    {
        FULL.len()
    } else if bytes.len() >= i + SHORT.len() && bytes[i..i + SHORT.len()].eq_ignore_ascii_case(SHORT)
    {
        SHORT.len()
    } else {
        return None;
    };
    i += name_len;
    match bytes.get(i) {
        Some(&b'>') => Some((i + 1, closing)),
        Some(c) if c.is_ascii_whitespace() => {
            let rest = &s[i..];
            let gt = rest.find('>')?;
            if rest[..gt].contains('<') {
                return None;
            }
            Some((i + gt + 1, closing))
        }
        _ => None,
    }
}

/// True if `s` (starting with `<` and containing no `>`) could still grow
/// into a think tag once more streamed bytes arrive.
pub(crate) fn could_be_think_tag_prefix(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.first() != Some(&b'<') || s.contains('>') {
        return false;
    }
    let mut rest = &bytes[1..];
    if rest.first() == Some(&b'/') {
        rest = &rest[1..];
    }
    const SHORT: &[u8] = b"think";
    const ING: &[u8] = b"ing";
    let head_len = rest.len().min(SHORT.len());
    if !rest[..head_len].eq_ignore_ascii_case(&SHORT[..head_len]) {
        return false;
    }
    if rest.len() <= SHORT.len() {
        return true;
    }
    let tail = &rest[SHORT.len()..];
    if tail[0].is_ascii_whitespace() {
        return true;
    }
    let ing_len = tail.len().min(ING.len());
    if !tail[..ing_len].eq_ignore_ascii_case(&ING[..ing_len]) {
        return false;
    }
    if tail.len() <= ING.len() {
        return true;
    }
    tail[ING.len()].is_ascii_whitespace()
}

/// Remove `<think>...</think>` reasoning spans (reasoning models like qwen3
/// and deepseek-r1 emit them).
///
/// * Closed blocks are removed wherever they appear.
/// * A stray closing tag (opener lost) keeps only text after the LAST closer.
/// * An unclosed opener drops everything from the opener onward — and if
///   nothing precedes it, everything is reasoning by construction, so the
///   result is empty (rejected later by validation rather than risking a
///   commit built from reasoning prose).
fn strip_think_blocks(text: &str) -> String {
    let mut tags: Vec<(usize, usize, bool)> = Vec::new();
    let mut i = 0;
    while let Some(off) = text[i..].find('<') {
        let pos = i + off;
        if let Some((len, closing)) = think_tag_at(&text[pos..]) {
            tags.push((pos, pos + len, closing));
            i = pos + len;
        } else {
            i = pos + 1;
        }
    }
    if tags.is_empty() {
        return text.to_string();
    }

    let mut result = String::new();
    let mut cursor = 0;
    let mut depth = 0usize;
    for (start, end, closing) in tags {
        if closing {
            if depth > 0 {
                depth -= 1;
                if depth == 0 {
                    cursor = end;
                }
            } else {
                // Stray closer: everything before it was reasoning.
                result.clear();
                cursor = end;
            }
        } else {
            if depth == 0 {
                result.push_str(&text[cursor..start]);
            }
            depth += 1;
        }
    }
    if depth > 0 {
        // Unclosed opener: drop from the opener to the end.
        if result.trim().is_empty() {
            return String::new();
        }
        return result;
    }
    result.push_str(&text[cursor..]);
    result
}

/// Strip one pair of matching quotes wrapping the whole message.
fn strip_wrapping_quotes(s: &str) -> &str {
    for q in ['"', '\''] {
        if s.len() >= 2 && s.starts_with(q) && s.ends_with(q) {
            return s[1..s.len() - 1].trim();
        }
    }
    s
}

/// Clean LLM output: strip reasoning blocks, wrapping quotes, code fences,
/// preamble, and trailing commentary. Finds the first line that looks like a
/// conventional commit and returns from there, discarding any surrounding
/// prose the model may have added.
pub fn clean_message(raw: &str) -> String {
    let without_think = strip_think_blocks(raw.trim());
    let raw = strip_wrapping_quotes(without_think.trim());

    // Strip markdown code fences if the whole message is wrapped
    let unwrapped = if raw.starts_with("```") {
        let inner = raw
            .strip_prefix("```")
            .unwrap_or(raw)
            .trim_start_matches(|c: char| c != '\n')
            .trim_start_matches('\n');
        inner.strip_suffix("```").unwrap_or(inner).trim()
    } else {
        raw
    };

    let lines: Vec<&str> = unwrapped.lines().collect();

    // Find first line that starts with a conventional commit type
    if let Some(start) = lines.iter().position(|line| is_commit_line(line)) {
        // Take from the commit line onwards
        let mut result: Vec<&str> = Vec::new();
        for &line in &lines[start..] {
            // Drop stray fence lines (a fence opened before the preamble
            // was stripped leaves its closing ``` behind)
            if line.trim().starts_with("```") {
                continue;
            }
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
    TYPES
        .iter()
        .any(|t| lower.starts_with(&format!("{t}:")) || lower.starts_with(&format!("{t}(")))
}

pub fn is_conventional_commit_message(message: &str) -> bool {
    message.lines().next().map(is_commit_line).unwrap_or(false)
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
        assert!(prompt.contains("imperative"), "Should mention 'imperative'");
        assert!(
            prompt.contains("max 72 chars"),
            "Should mention 'max 72 chars'"
        );
    }

    #[test]
    fn default_prompt_targets_primary_change_with_bullet_body() {
        let config = Config::default();
        let prompt = build_system_prompt(&config, None);
        assert!(
            prompt.contains("primary change"),
            "Prompt should anchor type and subject on the primary change: {prompt}"
        );
        assert!(
            prompt.contains("2-6 \"- \" bullets"),
            "Prompt should request bullet body for multi-concern commits: {prompt}"
        );
        assert!(
            prompt.contains("vague subjects"),
            "Prompt should ban vague subjects: {prompt}"
        );
        assert!(
            prompt.contains("never invent changes"),
            "Prompt should forbid inventing changes: {prompt}"
        );
    }

    #[test]
    fn default_prompt_explains_truncated_input_and_low_signal() {
        let config = Config::default();
        let prompt = build_system_prompt(&config, None);
        assert!(
            prompt.contains("truncated or omitted"),
            "Prompt should explain possibly-partial hunks: {prompt}"
        );
        assert!(
            prompt.contains("(low-signal)"),
            "Prompt should explain low-signal markers: {prompt}"
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
        assert!(
            prompt.contains("- Format: <type>: <subject>"),
            "Prompt should allow unscoped conventional commits when scope is None: {prompt}"
        );
        assert!(
            !prompt.contains("<type>(<scope>): <subject>"),
            "Prompt should not require a scope when none was detected: {prompt}"
        );
    }

    #[test]
    fn prompt_uses_scoped_format_when_scope_is_detected() {
        let config = Config::default();
        let prompt = build_system_prompt(&config, Some("auth"));
        assert!(
            prompt.contains("- Format: <type>(<scope>): <subject>"),
            "Prompt should prefer scoped format when scope is detected: {prompt}"
        );
    }

    // 4. Emoji instruction present when emoji=true
    #[test]
    fn prompt_includes_emoji_instruction() {
        let config = Config {
            emoji: true,
            ..Config::default()
        };
        let prompt = build_system_prompt(&config, None);
        assert!(
            prompt.contains("emoji"),
            "Prompt should contain 'emoji' when emoji is enabled"
        );
    }

    // 5. No emoji mention when emoji=false
    #[test]
    fn prompt_omits_emoji_when_disabled() {
        let config = Config {
            emoji: false,
            ..Config::default()
        };
        let prompt = build_system_prompt(&config, None);
        assert!(
            !prompt.contains("emoji"),
            "Prompt should not contain 'emoji' when emoji is disabled"
        );
    }

    // 6. Language instruction included when language is not English
    #[test]
    fn prompt_includes_language_when_not_english() {
        let config = Config {
            language: "ja".to_string(),
            ..Config::default()
        };
        let prompt = build_system_prompt(&config, None);
        assert!(
            prompt.contains("ja"),
            "Prompt should contain the language code 'ja'"
        );
    }

    #[test]
    fn prompt_includes_one_line_instruction() {
        let config = Config {
            one_line: true,
            ..Config::default()
        };
        let prompt = build_system_prompt(&config, None);
        assert!(
            prompt.contains("single-line"),
            "should mention single-line: {prompt}"
        );
        assert!(
            !prompt.contains("bullets"),
            "one_line should not include body guidance: {prompt}"
        );
    }

    // 7. Basic sanity: prompt is not empty with default config
    #[test]
    fn prompt_includes_user_context() {
        let config = Config::default();
        let prompt = build_system_prompt(&config, None);
        assert!(!prompt.is_empty(), "Prompt should not be empty");
    }

    #[test]
    fn prompt_stays_token_light() {
        let config = Config::default();
        let prompt = build_system_prompt(&config, Some("auth"));
        assert!(
            crate::token::estimate_tokens(&prompt) < 500,
            "System prompt must stay well under 500 tokens; it competes with the diff budget"
        );
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
        let raw =
            "feat(ui): add dark mode toggle\n\n(Note: I used feat because this is a new feature.)";
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
        assert_eq!(
            clean_message(raw),
            "fix: resolve null pointer on empty input"
        );
    }

    // --- think-block stripping ---

    #[test]
    fn clean_strips_closed_think_block() {
        let raw = "<think>\nThe diff adds an endpoint. Maybe feat: something else?\n</think>\n\nfeat(auth): add login endpoint";
        assert_eq!(clean_message(raw), "feat(auth): add login endpoint");
    }

    #[test]
    fn clean_strips_multiple_think_blocks() {
        let raw = "<think>first</think>fix: handle empty input<think>second</think>";
        assert_eq!(clean_message(raw), "fix: handle empty input");
    }

    #[test]
    fn clean_strips_uppercase_thinking_block() {
        let raw = "<THINKING>reasoning here</THINKING>\nchore: bump dependencies";
        assert_eq!(clean_message(raw), "chore: bump dependencies");
    }

    #[test]
    fn clean_unclosed_leading_think_returns_empty() {
        // A commit-looking line INSIDE unclosed reasoning must not be
        // extracted — everything after an unclosed leading opener is
        // reasoning by construction.
        let raw = "<think>\nmaybe this could be\nfix: adjust parser\nbecause the hunks show...";
        assert_eq!(clean_message(raw), "");
    }

    #[test]
    fn clean_unclosed_think_after_message_keeps_message() {
        let raw = "fix: adjust parser\n<think>leftover reasoning that never closes";
        assert_eq!(clean_message(raw), "fix: adjust parser");
    }

    #[test]
    fn clean_stray_closer_keeps_text_after_last() {
        let raw = "streamed-away reasoning tail</think>\nfeat: add retry ladder";
        assert_eq!(clean_message(raw), "feat: add retry ladder");
    }

    #[test]
    fn clean_strips_wrapping_quotes() {
        assert_eq!(clean_message("\"feat: add x\""), "feat: add x");
        assert_eq!(clean_message("'fix: repair y'"), "fix: repair y");
    }

    #[test]
    fn clean_think_fence_and_preamble_combined() {
        let raw = "<think>hmm</think>\nHere is the message:\n```\nfeat(api): add rate limiting\n```";
        assert_eq!(clean_message(raw), "feat(api): add rate limiting");
    }

    // --- tag matcher ---

    #[test]
    fn think_tag_matcher_accepts_variants() {
        assert_eq!(think_tag_at("<think>"), Some((7, false)));
        assert_eq!(think_tag_at("</think>"), Some((8, true)));
        assert_eq!(think_tag_at("<THINKING>"), Some((10, false)));
        assert_eq!(think_tag_at("<think type=\"x\">rest"), Some((16, false)));
        assert_eq!(think_tag_at("<thinks>"), None);
        assert_eq!(think_tag_at("<thought>"), None);
        assert_eq!(think_tag_at("plain text"), None);
    }

    #[test]
    fn think_tag_prefix_detection() {
        assert!(could_be_think_tag_prefix("<"));
        assert!(could_be_think_tag_prefix("<th"));
        assert!(could_be_think_tag_prefix("</thin"));
        assert!(could_be_think_tag_prefix("<thinkin"));
        assert!(could_be_think_tag_prefix("<think attr=\"unfinished"));
        assert!(!could_be_think_tag_prefix("<div"));
        assert!(!could_be_think_tag_prefix("<thinkx"));
        assert!(!could_be_think_tag_prefix("<think>")); // complete, not a prefix
    }
}
