use anyhow::Result;
use std::io::{self, BufRead, Write};
use crate::config;

pub async fn run() -> Result<()> {
    let mut stdout = io::stdout();
    let stdin = io::stdin();

    eprintln!("acm setup\n");

    // 1. Provider
    eprint!("Provider [ollama/openai] (default: ollama): ");
    stdout.flush()?;
    let provider = read_line_or_default(&stdin, "ollama")?;

    // 2. Model
    let default_model = if provider == "ollama" { "llama3" } else { "gpt-4o" };
    eprint!("Model (default: {default_model}): ");
    stdout.flush()?;
    let model = read_line_or_default(&stdin, default_model)?;

    // 3. API URL
    let default_url = if provider == "ollama" {
        "http://localhost:11434"
    } else {
        "https://api.openai.com"
    };
    eprint!("API URL (default: {default_url}): ");
    stdout.flush()?;
    let api_url = read_line_or_default(&stdin, default_url)?;

    // 4. API key (for non-ollama)
    let api_key = if provider != "ollama" {
        eprint!("API key: ");
        stdout.flush()?;
        let key = read_line(&stdin)?;
        if key.is_empty() {
            eprintln!("warning: no API key set. Set it later with: acm config set api_key=<key>");
            None
        } else {
            Some(key)
        }
    } else {
        None
    };

    // 5. Test connection
    eprint!("\nTesting connection to {api_url}...");
    stdout.flush()?;
    let test_ok = test_connection(&provider, &api_url, api_key.as_deref()).await;
    if test_ok {
        eprintln!(" ok");
    } else {
        eprintln!(" failed (config saved anyway — check your settings)");
    }

    // 6. Save config
    let path = config::config_path();
    config::set_value(&path, "provider", &provider)?;
    config::set_value(&path, "model", &model)?;
    config::set_value(&path, "api_url", &api_url)?;
    if let Some(key) = &api_key {
        config::set_value(&path, "api_key", key)?;
    }

    eprintln!("\nConfig saved to {}", path.display());
    eprintln!("Run `acm` to generate your first commit message.");
    Ok(())
}

fn read_line(stdin: &io::Stdin) -> Result<String> {
    let mut line = String::new();
    stdin.lock().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

fn read_line_or_default(stdin: &io::Stdin, default: &str) -> Result<String> {
    let line = read_line(stdin)?;
    Ok(if line.is_empty() { default.to_string() } else { line })
}

async fn test_connection(provider: &str, api_url: &str, api_key: Option<&str>) -> bool {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap_or_default();

    let result = if provider == "ollama" {
        client.get(format!("{api_url}/api/tags")).send().await
    } else {
        let mut req = client.get(format!("{api_url}/v1/models"));
        if let Some(key) = api_key {
            req = req.header("Authorization", format!("Bearer {key}"));
        }
        req.send().await
    };

    result.map(|r| r.status().is_success()).unwrap_or(false)
}
