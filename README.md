# acm

A fast, single-binary Rust CLI that generates git commit messages using local or remote LLMs. Replacement for [opencommit](https://github.com/di-sukharev/opencommit).

**3.8MB static binary. No runtime. No Node.js. No Python.**

## Quick Start

```bash
# If Ollama is running with llama3, it just works:
acm

# Or configure first:
acm setup
```

## Install

### From source

```bash
cargo install --path .
```

### Manual

```bash
cargo build --release
cp target/release/acm ~/.local/bin/
```

## Usage

```
acm                          # generate commit message for staged changes
acm -y                       # skip confirmation, commit directly
acm -c "migrated to v2 api"  # provide context hint
acm --dry-run                # show message but don't commit
acm config set model=llama3  # configure
acm config show              # show current config
acm setup                    # guided setup: pick provider, model, test connection
acm hook install             # install prepare-commit-msg git hook
acm hook uninstall           # remove git hook
```

### Interactive Flow

```
$ acm
  Staged: 5 files (+142, -38)

feat(auth): add JWT token refresh endpoint

  [y]es  [e]dit  [r]egenerate  [n]o  >
```

Tokens stream live as the model generates. When complete:

- **y** / Enter -- commit with the message
- **e** -- open `$EDITOR` to modify
- **r** -- regenerate (new LLM call, same diff)
- **n** / Esc -- cancel

### Auto-Staging

If no files are staged but changes exist, acm offers to stage everything:

```
$ acm
  No staged changes, but unstaged changes detected.

  Stage all changes? [y/n] >
```

With `-y`, staging is automatic.

### Push After Commit

After a successful commit, acm offers to push:

```
  Push? [y/n] >
```

## Providers

Two backends via one HTTP client:

| Provider | API | Default URL | Auth |
|----------|-----|-------------|------|
| **Ollama** (default) | `/api/chat` | `localhost:11434` | None |
| **OpenAI-compatible** | `/v1/chat/completions` | `api.openai.com` | Bearer token |

The OpenAI-compatible provider works with OpenAI, Groq, DeepSeek, OpenRouter, Azure, or any endpoint that speaks the same protocol.

### Examples

```bash
# Local Ollama (default, zero config)
acm

# OpenAI
acm config set provider=openai
acm config set model=gpt-4o
acm config set api_key=sk-...

# Groq
acm config set provider=openai
acm config set model=llama-3.3-70b-versatile
acm config set api_url=https://api.groq.com/openai
acm config set api_key=gsk_...

# Env var overrides (one-off)
ACM_MODEL=gemma acm --dry-run
```

## Configuration

Config file at `~/.config/acm/config.toml`:

```toml
provider = "ollama"
model = "llama3"
# api_url = "http://localhost:11434"
# api_key = ""
max_input_tokens = 4096
emoji = false
one_line = false
language = "en"
diff_mode = "auto"
```

### Layering

TOML file defaults -> environment variables (`ACM_PROVIDER`, `ACM_MODEL`, etc.) -> CLI flags.

### Options

| Option | Default | Description |
|--------|---------|-------------|
| `provider` | `ollama` | `ollama` or `openai` |
| `model` | `llama3` | Model name |
| `api_url` | per-provider | API endpoint |
| `api_key` | -- | API key (required for openai) |
| `max_input_tokens` | `4096` | Context window budget for diff |
| `emoji` | `false` | Prefix commit subject with emoji |
| `one_line` | `false` | Force single-line messages (no body) |
| `language` | `en` | Commit message language |
| `diff_mode` | `auto` | `auto`, `full`, `compact`, or `stat` |

## How It Works

### Progressive Diff Reduction

The core improvement over opencommit. Instead of splitting large diffs into multiple LLM calls and concatenating messages, acm progressively reduces diff detail to fit the context window:

```
full diff (--unified=3)
  ↓ too large?
compact diff (--unified=0)
  ↓ too large?
stat only (--stat)
  ↓ too large?
truncated stat
```

Always one LLM call. Always one coherent message.

### Scope Detection

Commit scope is auto-detected from staged file paths:

1. Find the deepest common directory among all staged files
2. Strip common prefixes (`src/`, `pkg/`, `apps/`, etc.)
3. Use the remainder as scope -- or omit if files are scattered

```
src/auth/login.rs + src/auth/middleware.rs  →  feat(auth): ...
src/auth/login.rs + src/config/mod.rs      →  feat: ...
```

### Token Estimation

Uses a simple `bytes / 3.2` heuristic instead of tiktoken. Intentionally conservative for code. If the API rejects with a context-length error, acm silently retries with the next smaller diff mode (up to 2 retries).

### Message Cleaning

LLM output is post-processed to strip preamble ("Here is a commit message:"), markdown code fences, and trailing commentary. The cleaner finds the first line matching a conventional commit type and discards surrounding prose.

## Git Hook

```bash
acm hook install    # writes prepare-commit-msg hook
acm hook uninstall  # removes it (only if installed by acm)
```

The hook runs `acm --hook` to generate a message when you run `git commit`.

## Error Messages

Errors are actionable, not stack traces:

| Condition | Message |
|-----------|---------|
| Ollama not running | `cannot connect to Ollama at localhost:11434. Is it running?` |
| Model not found | `model "X" not found. Run "ollama pull X" or "acm config set model=<name>"` |
| No staged files | `no staged changes. Stage files with "git add" first.` |
| API key missing | `ACM_API_KEY not set. Run "acm config set api_key=<key>"` |
| Context exceeded | Silent retry with smaller diff (up to 2 retries) |

## vs. opencommit

| | opencommit | acm |
|---|---|---|
| Large diffs | Splits into N LLM calls, concatenates N messages | Progressive reduction: always 1 call, 1 message |
| Size | 4.6MB JS + Node.js runtime | 3.8MB static binary |
| Token counting | tiktoken WASM (1.2MB) | `bytes/3.2` + retry on rejection |
| Providers | 12 bundled SDKs | 1 HTTP client, 2 API formats |
| Streaming | Blocks until complete | Streams tokens live |
| Prompt | 800+ tokens with examples | Minimal prompt, max room for diff |
| Config | INI, scattered validators | Typed TOML, validated at load |
| Local model errors | Generic | Actionable: "Is Ollama running?" |

## Building

```bash
cargo build --release    # optimized binary at target/release/acm
cargo test               # run all tests
cargo clippy             # lint
```

## License

MIT
