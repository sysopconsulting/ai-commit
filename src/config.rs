use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub provider: String,
    pub model: String,
    pub api_url: Option<String>,
    pub api_key: Option<String>,
    pub max_input_tokens: usize,
    pub emoji: bool,
    pub one_line: bool,
    pub language: String,
    pub diff_mode: String,
}

const CONFIG_KEYS: &[&str] = &[
    "provider",
    "model",
    "api_url",
    "api_key",
    "max_input_tokens",
    "emoji",
    "one_line",
    "language",
    "diff_mode",
];

impl Default for Config {
    fn default() -> Self {
        Config {
            provider: "ollama".to_string(),
            model: "llama3".to_string(),
            api_url: None,
            api_key: None,
            max_input_tokens: 4096,
            emoji: false,
            one_line: false,
            language: "en".to_string(),
            diff_mode: "auto".to_string(),
        }
    }
}

/// Returns ~/.config/acm/config.toml
pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("acm")
        .join("config.toml")
}

/// Load config from a specific path. Returns defaults if file is missing.
/// Returns a clear error if the file exists but is invalid TOML.
pub fn load_from_path(path: &PathBuf) -> Result<Config> {
    if !path.exists() {
        return Ok(Config::default());
    }
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read config file: {}", path.display()))?;
    let cfg: Config = toml::from_str(&content)
        .map_err(|e| anyhow::anyhow!("Invalid TOML in config file: {}: {e}", path.display()))?;
    validate(cfg)
}

/// Load config from default path + apply env overrides.
pub fn load() -> Result<Config> {
    let path = config_path();
    let mut cfg = load_from_path(&path)?;
    apply_env_overrides(&mut cfg);
    validate(cfg)
}

/// Apply all ACM_* environment variable overrides to the config.
pub fn apply_env_overrides(cfg: &mut Config) {
    let overrides: &[(&str, Option<String>)] = &[
        ("provider", std::env::var("ACM_PROVIDER").ok()),
        ("model", std::env::var("ACM_MODEL").ok()),
        ("api_url", std::env::var("ACM_API_URL").ok()),
        ("api_key", std::env::var("ACM_API_KEY").ok()),
        ("diff_mode", std::env::var("ACM_DIFF_MODE").ok()),
        ("language", std::env::var("ACM_LANGUAGE").ok()),
        (
            "max_input_tokens",
            std::env::var("ACM_MAX_INPUT_TOKENS").ok(),
        ),
        ("emoji", std::env::var("ACM_EMOJI").ok()),
        ("one_line", std::env::var("ACM_ONE_LINE").ok()),
    ];
    for (key, value) in overrides {
        apply_env_override(cfg, key, value.as_deref());
    }
}

fn validate(cfg: Config) -> Result<Config> {
    match cfg.provider.as_str() {
        "ollama" | "openai" => {}
        other => anyhow::bail!("unknown provider: {other}. Use \"ollama\" or \"openai\"."),
    }

    match cfg.diff_mode.as_str() {
        "auto" | "full" | "compact" | "budgeted" | "stat" => {}
        other => anyhow::bail!(
            "unknown diff_mode: {other}. Use \"auto\", \"full\", \"compact\", \"budgeted\", or \"stat\"."
        ),
    }

    if cfg.max_input_tokens == 0 {
        anyhow::bail!("max_input_tokens must be greater than 0");
    }

    Ok(cfg)
}

/// Apply a single environment variable override to the config (testable).
pub fn apply_env_override(cfg: &mut Config, key: &str, value: Option<&str>) {
    let Some(v) = value else { return };
    match key {
        "provider" => cfg.provider = v.to_string(),
        "model" => cfg.model = v.to_string(),
        "api_url" => cfg.api_url = Some(v.to_string()),
        "api_key" => cfg.api_key = Some(v.to_string()),
        "diff_mode" => cfg.diff_mode = v.to_string(),
        "language" => cfg.language = v.to_string(),
        "max_input_tokens" => {
            if let Ok(n) = v.parse::<usize>() {
                cfg.max_input_tokens = n;
            }
        }
        "emoji" => {
            if let Ok(b) = v.parse::<bool>() {
                cfg.emoji = b;
            }
        }
        "one_line" => {
            if let Ok(b) = v.parse::<bool>() {
                cfg.one_line = b;
            }
        }
        _ => {}
    }
}

/// Read existing TOML file at path, set key=value, write back.
/// Creates parent directories if missing.
/// Preserves types: bool for "true"/"false", integer for numeric strings, string otherwise.
pub fn set_value(path: &PathBuf, key: &str, value: &str) -> Result<()> {
    if !CONFIG_KEYS.contains(&key) {
        anyhow::bail!("unknown config key: {key}");
    }

    // Create parent dirs if needed
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directories for: {}", path.display()))?;
    }

    // Read existing TOML or start fresh
    let existing = if path.exists() {
        std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?
    } else {
        String::new()
    };

    let mut table: toml::Table = if existing.is_empty() {
        toml::Table::new()
    } else {
        toml::from_str(&existing)
            .with_context(|| format!("Invalid TOML in config file: {}", path.display()))?
    };

    // Determine value type: bool, integer, or string
    let toml_value = if value == "true" {
        toml::Value::Boolean(true)
    } else if value == "false" {
        toml::Value::Boolean(false)
    } else if let Ok(n) = value.parse::<i64>() {
        toml::Value::Integer(n)
    } else {
        toml::Value::String(value.to_string())
    };

    table.insert(key.to_string(), toml_value);

    let output =
        toml::to_string_pretty(&table).with_context(|| "Failed to serialize config to TOML")?;
    let cfg: Config =
        toml::from_str(&output).with_context(|| format!("Invalid config value for key: {key}"))?;
    validate(cfg)?;

    std::fs::write(path, output)
        .with_context(|| format!("Failed to write config file: {}", path.display()))?;

    Ok(())
}

/// Return a human-readable display string, masking api_key.
pub fn display(cfg: &Config) -> String {
    let api_key_display = match &cfg.api_key {
        Some(_) => "(set)".to_string(),
        None => "(not set)".to_string(),
    };
    let api_url_display = cfg.api_url.as_deref().unwrap_or("(not set)").to_string();

    format!(
        "provider         = {}\nmodel            = {}\napi_url          = {}\napi_key          = {}\nmax_input_tokens = {}\nemoji            = {}\none_line         = {}\nlanguage         = {}\ndiff_mode        = {}",
        cfg.provider,
        cfg.model,
        api_url_display,
        api_key_display,
        cfg.max_input_tokens,
        cfg.emoji,
        cfg.one_line,
        cfg.language,
        cfg.diff_mode,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;
    use tempfile::TempDir;

    // ── 1. Default values are Ollama-first ──────────────────────────────────

    #[test]
    fn test_default_config_ollama_first() {
        let cfg = Config::default();
        assert_eq!(cfg.provider, "ollama");
        assert_eq!(cfg.model, "llama3");
        assert!(cfg.api_url.is_none());
        assert!(cfg.api_key.is_none());
        assert_eq!(cfg.max_input_tokens, 4096);
        assert!(!cfg.emoji);
        assert!(!cfg.one_line);
        assert_eq!(cfg.language, "en");
        assert_eq!(cfg.diff_mode, "auto");
    }

    // ── 2. Partial TOML parsing — unset fields fall back to defaults ────────

    #[test]
    fn test_partial_toml_parsing() {
        let toml_str = r#"
provider = "openai"
model = "gpt-4o"
"#;
        let cfg: Config = toml::from_str(toml_str).expect("should parse");
        assert_eq!(cfg.provider, "openai");
        assert_eq!(cfg.model, "gpt-4o");
        // Fields not in the TOML must come from Default
        assert_eq!(cfg.max_input_tokens, 4096);
        assert_eq!(cfg.language, "en");
        assert!(!cfg.emoji);
    }

    // ── 3. Loading from a TOML file ─────────────────────────────────────────

    #[test]
    fn test_load_from_toml_file() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(
            f,
            r#"provider = "openai"
model = "gpt-4o"
api_key = "sk-test"
max_input_tokens = 8192
emoji = true
"#
        )
        .unwrap();
        let cfg = load_from_path(&f.path().to_path_buf()).unwrap();
        assert_eq!(cfg.provider, "openai");
        assert_eq!(cfg.model, "gpt-4o");
        assert_eq!(cfg.api_key.as_deref(), Some("sk-test"));
        assert_eq!(cfg.max_input_tokens, 8192);
        assert!(cfg.emoji);
    }

    // ── 4. Missing file returns defaults ────────────────────────────────────

    #[test]
    fn test_missing_file_returns_defaults() {
        let path = PathBuf::from("/tmp/acm_nonexistent_config_xyz.toml");
        let cfg = load_from_path(&path).unwrap();
        assert_eq!(cfg.provider, "ollama");
        assert_eq!(cfg.model, "llama3");
    }

    // ── 5. Env var overrides ─────────────────────────────────────────────────

    #[test]
    fn test_env_var_overrides() {
        let mut cfg = Config::default();
        apply_env_override(&mut cfg, "provider", Some("openai"));
        apply_env_override(&mut cfg, "model", Some("gpt-4o-mini"));
        apply_env_override(&mut cfg, "api_key", Some("sk-test-123"));
        apply_env_override(&mut cfg, "max_input_tokens", Some("16384"));
        apply_env_override(&mut cfg, "emoji", Some("true"));
        apply_env_override(&mut cfg, "one_line", Some("true"));
        apply_env_override(&mut cfg, "language", Some("ro"));
        apply_env_override(&mut cfg, "diff_mode", Some("staged"));

        assert_eq!(cfg.provider, "openai");
        assert_eq!(cfg.model, "gpt-4o-mini");
        assert_eq!(cfg.api_key.as_deref(), Some("sk-test-123"));
        assert_eq!(cfg.max_input_tokens, 16384);
        assert!(cfg.emoji);
        assert!(cfg.one_line);
        assert_eq!(cfg.language, "ro");
        assert_eq!(cfg.diff_mode, "staged");
    }

    #[test]
    fn test_env_override_none_is_noop() {
        let mut cfg = Config::default();
        apply_env_override(&mut cfg, "provider", None);
        assert_eq!(cfg.provider, "ollama"); // unchanged
    }

    // ── 6. Invalid TOML gives a clear error mentioning the file path ─────────

    #[test]
    fn test_invalid_toml_error_mentions_file() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "this is not = valid toml ][[[").unwrap();
        let err = load_from_path(&f.path().to_path_buf()).unwrap_err();
        let msg = err.to_string();
        // The error chain must mention the file path
        let path_str = f.path().to_string_lossy().to_string();
        assert!(
            msg.contains(&path_str),
            "Error should mention the file path. Got: {msg}"
        );
    }

    // ── 7. set_value writes key=value to TOML ───────────────────────────────

    #[test]
    fn test_set_value_writes_to_toml() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");

        set_value(&path, "provider", "openai").unwrap();
        set_value(&path, "model", "gpt-4o").unwrap();
        set_value(&path, "emoji", "true").unwrap();
        set_value(&path, "max_input_tokens", "8192").unwrap();

        let cfg = load_from_path(&path).unwrap();
        assert_eq!(cfg.provider, "openai");
        assert_eq!(cfg.model, "gpt-4o");
        assert!(cfg.emoji);
        assert_eq!(cfg.max_input_tokens, 8192);
    }

    #[test]
    fn test_set_value_preserves_types() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");

        set_value(&path, "emoji", "true").unwrap();
        set_value(&path, "max_input_tokens", "4096").unwrap();
        set_value(&path, "provider", "ollama").unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        // bool should not be quoted
        assert!(content.contains("emoji = true"), "Expected bool: {content}");
        // integer should not be quoted
        assert!(
            content.contains("max_input_tokens = 4096"),
            "Expected integer: {content}"
        );
        // string should be quoted
        assert!(
            content.contains("provider = \"ollama\""),
            "Expected quoted string: {content}"
        );
    }

    // ── 8. set_value creates file and parent dirs if missing ────────────────

    #[test]
    fn test_set_value_creates_parent_dirs() {
        let dir = TempDir::new().unwrap();
        let nested_path = dir.path().join("deep").join("nested").join("config.toml");

        // The parent dirs do not exist yet
        assert!(!nested_path.parent().unwrap().exists());

        set_value(&nested_path, "provider", "ollama").unwrap();

        assert!(nested_path.exists(), "Config file should have been created");
        let cfg = load_from_path(&nested_path).unwrap();
        assert_eq!(cfg.provider, "ollama");
    }

    // ── 9. display() masks api_key ───────────────────────────────────────────

    #[test]
    fn test_display_masks_api_key() {
        let cfg = Config {
            api_key: Some("super-secret-key".to_string()),
            ..Config::default()
        };
        let out = display(&cfg);
        assert!(
            !out.contains("super-secret-key"),
            "api_key must not appear in display output"
        );
        assert!(out.contains("(set)"), "Should show '(set)' for api_key");
    }

    #[test]
    fn test_display_shows_not_set_when_no_api_key() {
        let cfg = Config::default();
        let out = display(&cfg);
        assert!(
            out.contains("(not set)"),
            "Should show '(not set)' when api_key is None"
        );
    }

    #[test]
    fn test_load_rejects_unknown_provider() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, r#"provider = "anthropic""#).unwrap();
        let err = load_from_path(&f.path().to_path_buf()).unwrap_err();
        assert!(
            err.to_string().contains("unknown provider"),
            "Expected provider validation error, got: {err}"
        );
    }

    #[test]
    fn test_budgeted_diff_mode_accepted() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");

        set_value(&path, "diff_mode", "budgeted").unwrap();
        let cfg = load_from_path(&path).unwrap();
        assert_eq!(cfg.diff_mode, "budgeted");
    }

    #[test]
    fn test_set_value_rejects_invalid_diff_mode() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");

        let err = set_value(&path, "diff_mode", "staged").unwrap_err();
        assert!(
            err.to_string().contains("unknown diff_mode"),
            "Expected diff_mode validation error, got: {err}"
        );
    }

    #[test]
    fn test_set_value_rejects_unknown_key() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");

        let err = set_value(&path, "modle", "gpt-4o").unwrap_err();
        assert!(
            err.to_string().contains("unknown config key"),
            "Expected unknown key validation error, got: {err}"
        );
    }

    #[test]
    fn test_load_rejects_unknown_key() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, r#"modle = "gpt-4o""#).unwrap();

        let err = load_from_path(&f.path().to_path_buf()).unwrap_err();
        assert!(
            err.to_string().contains("unknown field")
                || err.to_string().contains("unknown config key"),
            "Expected unknown key validation error, got: {err}"
        );
    }
}
