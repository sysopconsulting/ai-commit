use std::path::Path;

/// Auto-detect commit scope from staged file paths.
///
/// Returns `None` if:
/// - `files` is empty
/// - any file is at the root level (no directory component)
/// - there is no common directory prefix after stripping known top-level prefixes
///
/// Known prefixes that are stripped from the first path component:
/// `src`, `pkg`, `libs`, `apps`, `lib`, `packages`
pub fn detect_scope(files: &[String]) -> Option<String> {
    if files.is_empty() {
        return None;
    }

    // Collect directory components for every file (filename is excluded).
    let dir_parts: Vec<Vec<String>> = files
        .iter()
        .map(|f| {
            let path = Path::new(f);
            // parent() gives everything except the final component (the filename).
            match path.parent() {
                None => vec![],
                Some(p) if p == Path::new("") => vec![],
                Some(p) => p
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect(),
            }
        })
        .collect();

    // If any file is at the root (no directory), return None.
    if dir_parts.iter().any(|parts| parts.is_empty()) {
        return None;
    }

    // Find the common directory prefix across all files.
    let first = &dir_parts[0];
    let common_len = dir_parts.iter().skip(1).fold(first.len(), |acc, parts| {
        let shared = first
            .iter()
            .zip(parts.iter())
            .take_while(|(a, b)| a == b)
            .count();
        acc.min(shared)
    });

    let common: Vec<&str> = first[..common_len].iter().map(|s| s.as_str()).collect();

    // Strip a leading known prefix (first component only).
    const KNOWN_PREFIXES: &[&str] = &["src", "pkg", "libs", "apps", "lib", "packages"];
    let stripped: &[&str] = if common
        .first()
        .map(|c| KNOWN_PREFIXES.contains(c))
        .unwrap_or(false)
    {
        &common[1..]
    } else {
        &common
    };

    if stripped.is_empty() {
        return None;
    }

    Some(stripped.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(strs: &[&str]) -> Vec<String> {
        strs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn empty_files_returns_none() {
        assert_eq!(detect_scope(&[]), None);
    }

    #[test]
    fn root_files_returns_none() {
        assert_eq!(detect_scope(&s(&["README.md", "Cargo.toml"])), None);
    }

    #[test]
    fn single_file_in_src_subdir() {
        assert_eq!(
            detect_scope(&s(&["src/auth/login.rs"])),
            Some("auth".into())
        );
    }

    #[test]
    fn multiple_files_same_subdir() {
        assert_eq!(
            detect_scope(&s(&["src/auth/login.rs", "src/auth/middleware.rs"])),
            Some("auth".into())
        );
    }

    #[test]
    fn files_in_different_subdirs_under_src() {
        assert_eq!(
            detect_scope(&s(&["src/auth/login.rs", "src/config/mod.rs"])),
            None
        );
    }

    #[test]
    fn nested_scope() {
        assert_eq!(
            detect_scope(&s(&["src/llm/ollama.rs", "src/llm/openai.rs"])),
            Some("llm".into())
        );
    }

    #[test]
    fn strips_pkg_prefix() {
        assert_eq!(
            detect_scope(&s(&["pkg/api/handler.go", "pkg/api/routes.go"])),
            Some("api".into())
        );
    }

    #[test]
    fn mixed_root_and_subdir_returns_none() {
        assert_eq!(detect_scope(&s(&["src/auth/login.rs", "README.md"])), None);
    }

    #[test]
    fn only_src_returns_none() {
        assert_eq!(detect_scope(&s(&["src/main.rs"])), None);
    }

    #[test]
    fn apps_monorepo_prefix() {
        assert_eq!(
            detect_scope(&s(&["apps/web/index.ts", "apps/web/layout.ts"])),
            Some("web".into())
        );
    }

    #[test]
    fn deeply_nested_scope() {
        assert_eq!(
            detect_scope(&s(&[
                "src/llm/providers/ollama.rs",
                "src/llm/providers/openai.rs"
            ])),
            Some("llm/providers".into())
        );
    }
}
