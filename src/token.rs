pub fn estimate_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    (text.len() as f64 / 3.2).ceil() as usize
}

pub fn truncate_to_tokens(text: &str, max_tokens: usize) -> String {
    if estimate_tokens(text) <= max_tokens {
        return text.to_string();
    }
    let max_bytes = (max_tokens as f64 * 3.2) as usize;
    let truncated = &text[..max_bytes.min(text.len())];
    match truncated.rfind('\n') {
        Some(pos) => truncated[..=pos].to_string(),
        None => truncated.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_string_is_zero_tokens() {
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn short_string_estimate() {
        // "hello" = 5 bytes / 3.2 = 1.5625, ceil = 2
        assert_eq!(estimate_tokens("hello"), 2);
    }

    #[test]
    fn code_snippet_estimate() {
        let code = "fn main() {\n    println!(\"Hello, world!\");\n}\n";
        let est = estimate_tokens(code);
        // 45 bytes / 3.2 = 14.0625, ceil = 15
        assert_eq!(est, 15);
    }

    #[test]
    fn truncate_returns_full_if_fits() {
        let text = "short text";
        assert_eq!(truncate_to_tokens(text, 100), text);
    }

    #[test]
    fn truncate_cuts_to_fit() {
        let text = "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\n";
        let result = truncate_to_tokens(text, 5);
        assert!(estimate_tokens(&result) <= 5, "truncated should fit: {result}");
        assert!(!result.is_empty());
    }
}
