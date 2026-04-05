# acm - AI Commit Message Generator

A fast, single-binary Rust CLI that generates git commit messages using local or remote LLMs. Replacement for opencommit.

## CLI Interface

```
acm                          # generate commit message for staged changes
acm -y                       # skip confirmation, commit directly
acm -c "migrated to v2 api"  # provide context hint
acm --dry-run                # show message but don't commit
acm config set model=llama3  # configure
acm config show              # show current config
acm setup                    # guided config: pick provider, model, test connection
acm hook install             # install prepare-commit-msg git hook
acm hook uninstall           # remove git hook
```

## Providers

Two backends via one HTTP client (`reqwest`):

- **Ollama** native API (`/api/chat`) — default, no API key needed
- **OpenAI-compatible** (`/v1/chat/completions`) — covers OpenAI, Groq, DeepSeek, OpenRouter, Azure, any compatible endpoint

Streaming is the only code path. Both providers implement the same trait returning a token stream.

## Architecture

```
src/
  main.rs              # CLI parsing (clap), orchestration
  config/
    mod.rs             # TOML config + env var overlay
  git/
    mod.rs             # git operations (staged files, diff, commit)
  llm/
    mod.rs             # LlmProvider trait, diff-fitting strategy
    ollama.rs          # Ollama native API
    openai.rs          # OpenAI-compatible API
  prompt/
    mod.rs             # system prompt construction
```

### Provider Trait

```rust
trait LlmProvider {
    async fn chat_stream(&self, messages: Vec<Message>) -> Result<impl Stream<Item = String>>;
}
```

Both Ollama and OpenAI-compatible implement this. To display the full message, collect the stream. No separate non-streaming path.

### No Token Counting Library

Use char-based estimation: `string.len() / 4`. Intentionally conservative. If the estimate undershoots and the API rejects with a context-length error, retry with the next-smaller diff mode. This eliminates the 1.2MB tiktoken WASM dependency.

### Progressive Diff Reduction

The core improvement over opencommit. Never split diffs into multiple LLM calls. One call, one coherent message, always.

```
fn fit_diff(staged_files, max_tokens, forced_mode) -> String:
    if forced_mode != "auto":
        return get_diff(forced_mode)

    full = git_diff(--unified=3, staged_files)
    if estimate_tokens(full) <= max_tokens:
        return full

    compact = git_diff(--unified=0, staged_files)
    if estimate_tokens(compact) <= max_tokens:
        return compact

    stat = git_diff(--stat, staged_files)
    if estimate_tokens(stat) <= max_tokens:
        return stat

    // Absurd changeset — truncate stat to fit
    return truncate(stat, max_tokens)
```

If the API still rejects (estimation was off), catch the context-length error and retry one level down. Maximum 2 retries.

## Config

File at `~/.config/acm/config.toml`:

```toml
# Provider: "ollama" or "openai"
provider = "ollama"

# Model name
model = "llama3"

# API endpoint (provider-specific defaults if omitted)
# api_url = "http://localhost:11434"

# API key (not needed for Ollama)
# api_key = ""

# Max input tokens (context window estimate for diff fitting)
max_input_tokens = 4096

# Commit message style
emoji = false
one_line = false
language = "en"

# Diff mode: "auto", "full", "compact", "stat"
# "auto" uses progressive reduction (recommended)
diff_mode = "auto"
```

### Layering

TOML file defaults -> env var overrides (`ACM_<UPPER>`) -> CLI flags.

Config struct is a typed Rust struct, deserialized with serde. Invalid config produces a clear error at startup pointing to the exact field. No scattered validators, no migrations.

### Defaults

Ollama-first: provider=ollama, model=llama3, api_url=localhost:11434. Zero config if Ollama is running.

## Prompt

Short and token-efficient. No few-shot examples.

```
You are a git commit message generator. Write a concise conventional commit message for the following changes.

Rules:
- Format: <type>(<scope>): <subject>
- Types: fix, feat, refactor, docs, test, chore, style, perf, build, ci
- Subject: imperative, lowercase, no period, max 72 chars
- One line unless the changes are complex enough to warrant a body
{emoji_instruction}
{language_instruction}
{user_context}
```

The diff (at whatever detail level was selected) is sent as the user message.

## User Interaction

```
$ acm
  Staged: 5 files (+142, -38)

feat(auth): add JWT token refresh endpoint

  [y] commit  [e] edit  [r] regenerate  [n] cancel
```

The commit message streams live — characters appear as the model generates. When complete, the action prompt appears.

- **y** — commit with the message
- **e** — open `$EDITOR` with message pre-filled (fallback: inline edit)
- **r** — regenerate (same diff, new LLM call)
- **n** — exit, no commit

Flags:
- `-y` — skip prompt, commit directly (for scripts/hooks)
- `--dry-run` — stream the message, exit without committing

## Error Handling

Actionable messages, no stack traces:

| Condition | Message |
|-----------|---------|
| Ollama not running | `error: cannot connect to Ollama at localhost:11434. Is it running?` |
| Model not found | `error: model "llama3" not found. Run "ollama pull llama3" or "acm config set model=<name>"` |
| Context exceeded | Silent retry with next-smaller diff mode (up to 2 retries) |
| Network timeout | `error: request timed out after 30s` |
| No staged files | `error: no staged changes. Stage files with "git add" first.` |
| API key missing (openai) | `error: ACM_API_KEY not set. Run "acm config set api_key=<key>"` |

No interactive model pickers. No blocking setup wizards. Clear errors with fix instructions.

## Git Hook

```
acm hook install    # writes prepare-commit-msg hook
acm hook uninstall  # removes it
```

The hook runs `acm --hook` which writes the generated message as a comment in the commit message template, same proven pattern as opencommit.

## Dependencies

| Crate | Purpose |
|-------|---------|
| `clap` + derive | CLI parsing |
| `reqwest` + `rustls` | HTTP + streaming, no OpenSSL |
| `serde` + `toml` | Config deserialization |
| `tokio` (minimal features) | Async runtime for streaming |
| `crossterm` | Terminal input/colors, no ncurses |
| `futures` | Stream combinators for SSE parsing |

No per-provider SDK. No WASM. No git library (shell out to `git`). Single static binary.

**Target binary size:** 3-5MB.

**Cross-platform:** rustls for TLS, crossterm for terminal. Linux, macOS, Windows from one codebase.

## What opencommit Gets Wrong (and acm Fixes)

| opencommit problem | acm solution |
|--------------------|-------------|
| Splits large diffs into N LLM calls, concatenates N messages | Progressive diff reduction: always 1 call, 1 coherent message |
| 4.6MB JS bundle + Node.js runtime | 3-5MB static binary, no runtime |
| Tiktoken WASM for token counting (1.2MB) | char/4 estimation + retry on API rejection |
| Bundles all 12 provider SDKs | One HTTP client, two API formats |
| No streaming — blocks until complete | Streams tokens live |
| 800+ token prompt with fake examples | Minimal prompt, max room for diff |
| 2s hardcoded delay between chunk requests | No chunks, no delays |
| INI config, scattered validators | Typed TOML struct, validated at load |
| Generic errors for local models | Actionable errors: "Is Ollama running?", "Run ollama pull X" |
