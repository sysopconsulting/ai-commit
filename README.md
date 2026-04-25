# acm

`acm` is a small Rust CLI that generates commit messages from your staged Git
diff using either a local Ollama model or an OpenAI-compatible API.

It is meant to be the fast, single-binary alternative to Node-based commit
message generators: no runtime, no package manager, no SDK sprawl.

## Highlights

- Single native binary, about 4 MB in release builds
- Works with Ollama by default
- Supports OpenAI-compatible APIs with one HTTP client
- Streams the generated commit message live
- Lets you confirm, edit, regenerate, cancel, or auto-commit
- Handles large diffs by progressively reducing detail
- Detects conventional-commit scopes from staged file paths
- Can install a `prepare-commit-msg` hook
- Includes `--dry-run`, `--version`, and environment overrides

## Quick Start

If Ollama is running locally and has `llama3` available:

```bash
git add src/main.rs
acm
```

For guided setup:

```bash
acm setup
```

To check the installed version:

```bash
acm --version
```

## Installation

### From Source

```bash
cargo install --path .
```

### Manual Local Install

```bash
cargo build --release
cp target/release/acm ~/.local/bin/acm
acm --version
```

Make sure `~/.local/bin` is on your `PATH`.

## Usage

```bash
acm                          # generate a message for staged changes
acm -y                       # commit without confirmation
acm --dry-run                # generate and print, but do not commit
acm -c "migrated to v2 api"  # add extra context for the model
acm --version                # print version

acm setup                    # guided provider/model setup
acm config show              # print current config
acm config set model=llama3  # set one config value

acm hook install             # install prepare-commit-msg hook
acm hook uninstall           # remove acm's hook
```

## Interactive Workflow

With staged changes, `acm` first shows the staged diff in your pager. After you
quit the pager, the generated commit message streams once, then the action
prompt appears:

```text
$ acm
  Staged: 5 files (+142, -38)

# staged diff opens in $PAGER

feat(auth): add JWT token refresh endpoint
  [y]es  [e]dit  [r]egenerate  [n]o  >
```

Actions:

- `y` or Enter: commit with the generated message
- `e`: open `$EDITOR`, edit the message, then commit
- `r`: regenerate using the same diff and context
- `n`, Esc, or Ctrl-C: cancel

After a successful commit, `acm` asks whether to push:

```text
  Push? [y/n] >
```

## Auto-Staging

If nothing is staged but the working tree has changes, `acm` offers to stage all
changes. It prints a short preview first:

```text
$ acm
  No staged changes, but unstaged changes detected.

  .gitignore | 1 +
   1 file changed, 1 insertion(+)

  Stage all changes? [y/n] >
```

Untracked files are included in the preview:

```text
  README.md | untracked
```

With `-y`, this staging step is automatic:

```bash
acm -y
```

## Providers

`acm` supports two provider modes.

| Provider | API | Default URL | Auth |
| --- | --- | --- | --- |
| `ollama` | `/api/chat` | `http://localhost:11434` | None |
| `openai` | `/v1/chat/completions` | `https://api.openai.com` | Bearer token |

The `openai` provider is intentionally generic. It works with OpenAI and many
OpenAI-compatible services such as Groq, DeepSeek, OpenRouter, and local proxy
servers.

### Ollama

```bash
ollama pull llama3
acm config set provider=ollama
acm config set model=llama3
acm
```

### OpenAI

```bash
acm config set provider=openai
acm config set model=gpt-4o
acm config set api_key=sk-...
acm
```

### Groq Example

```bash
acm config set provider=openai
acm config set api_url=https://api.groq.com/openai
acm config set model=llama-3.3-70b-versatile
acm config set api_key=gsk_...
```

## Configuration

Config lives at:

```text
~/.config/acm/config.toml
```

Example:

```toml
provider = "ollama"
model = "llama3"
api_url = "http://localhost:11434"
max_input_tokens = 4096
emoji = false
one_line = false
language = "en"
diff_mode = "auto"
```

For OpenAI-compatible providers, also set:

```toml
api_key = "sk-..."
```

### Config Values

| Key | Default | Description |
| --- | --- | --- |
| `provider` | `ollama` | `ollama` or `openai` |
| `model` | `llama3` | Model name sent to the provider |
| `api_url` | provider-specific | Base API URL |
| `api_key` | unset | Required for `openai` provider |
| `max_input_tokens` | `4096` | Diff budget before reduction |
| `emoji` | `false` | Ask for an emoji in the subject |
| `one_line` | `false` | Force a single-line message |
| `language` | `en` | Language hint for the message |
| `diff_mode` | `auto` | `auto`, `full`, `compact`, or `stat` |

Invalid `provider`, `diff_mode`, and zero token budgets are rejected when config
is loaded or changed.

### Environment Overrides

Every config value can be overridden for one command:

```bash
ACM_MODEL=gemma acm --dry-run
ACM_PROVIDER=openai ACM_API_KEY=sk-... acm
ACM_DIFF_MODE=stat acm -c "large dependency update"
```

Supported variables:

```text
ACM_PROVIDER
ACM_MODEL
ACM_API_URL
ACM_API_KEY
ACM_MAX_INPUT_TOKENS
ACM_EMOJI
ACM_ONE_LINE
ACM_LANGUAGE
ACM_DIFF_MODE
```

## Git Hook

Install the hook:

```bash
acm hook install
```

Remove it:

```bash
acm hook uninstall
```

The hook runs during `git commit` and writes a generated message into Git's
commit message file. It is intentionally conservative:

- It skips commits created by `acm` itself, avoiding recursive generation.
- It skips manual `git commit -m ...` messages.
- It skips merge, squash, template, and amend source messages.
- It resolves the hook path through Git, so linked worktrees are supported.
- It refuses to overwrite a non-`acm` hook.

## How Diff Selection Works

Large diffs are reduced in stages so the model gets one coherent prompt instead
of several partial requests.

```text
full diff
  ↓ too large
compact diff with --unified=0
  ↓ too large
stat only
  ↓ too large
truncated stat
```

You can force a mode:

```bash
acm config set diff_mode=full
acm config set diff_mode=compact
acm config set diff_mode=stat
acm config set diff_mode=auto
```

If the provider still rejects the prompt for context length, `acm` retries with
the next smaller diff mode up to two times.

## Scope Detection

`acm` derives a conventional-commit scope from staged file paths.

```text
src/auth/login.rs + src/auth/session.rs  -> feat(auth): ...
src/llm/ollama.rs + src/llm/openai.rs    -> refactor(llm): ...
src/auth/login.rs + README.md            -> feat: ...
```

Common top-level prefixes such as `src`, `pkg`, `apps`, `lib`, `libs`, and
`packages` are stripped from the scope.

## Message Cleanup

Models sometimes add prose around the answer. `acm` cleans common cases:

- Markdown fences
- "Here is a commit message" preambles
- Trailing explanatory notes

If cleanup changes the streamed output, `acm` prints the cleaned message before
continuing.

## Troubleshooting

| Symptom | What to check |
| --- | --- |
| `cannot connect to Ollama` | Start Ollama and verify `api_url` |
| `model ... not found` | Run `ollama pull <model>` or set another model |
| `ACM_API_KEY not set` | Set `api_key` or `ACM_API_KEY` for `openai` |
| No staged files | Run `git add`, or accept auto-staging |
| Provider rejects context | Use `diff_mode=compact` or `diff_mode=stat` |
| Editor does not open | Set `$EDITOR`, for example `export EDITOR="vim"` |
| Hook does not install | Check for an existing non-`acm` hook |

## Development

```bash
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
cargo build --release
```

Run the local binary:

```bash
target/release/acm --version
target/release/acm --dry-run
```

Install the release build locally:

```bash
cp target/release/acm ~/.local/bin/acm
~/.local/bin/acm --version
```

## Release Notes

Current package version: `0.2.2`.

Local tags use `vX.Y.Z`, for example:

```bash
git tag -a v0.2.2 -m "v0.2.2"
```

## License

MIT
