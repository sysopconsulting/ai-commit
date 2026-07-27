#!/usr/bin/env bash
#
# Local end-to-end smoke gate for acm.
#
# Rebuilds three real changesets from this repo's history in throwaway repos
# (each from the target commit's parent, with the commit's patch staged), runs
# the release binary with --dry-run, and asserts only stable properties:
#
#   - stdout's first line is a well-formed conventional commit header
#   - that line is <= 72 characters
#   - the body, if any, is "- " bullets rather than a prose paragraph
#
# Exact wording is deliberately NOT asserted: the model tag is mutable and
# sampling is nondeterministic, so this is a smoke gate rather than a golden
# output test. Requires a running Ollama with the model available.
#
# Usage:  scripts/smoke-local.sh [model]
set -uo pipefail

SRC=$(git rev-parse --show-toplevel)
ACM="$SRC/target/release/acm"
MODEL=${1:-qwen3-coder:30b}

[ -x "$ACM" ] || { echo "build first: cargo build --release" >&2; exit 2; }

# Every ACM_* setting is pinned so the result never depends on the developer's
# ~/.config/acm/config.toml.
export ACM_PROVIDER=ollama
export ACM_API_URL=${ACM_API_URL:-http://localhost:11434}
export ACM_MODEL="$MODEL"
export ACM_MAX_INPUT_TOKENS=4096
export ACM_DIFF_MODE=auto
export ACM_EMOJI=false
export ACM_ONE_LINE=false
export ACM_LANGUAGE=en

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

# name   parent   commit
CASES=(
  "big   0822cd8  fcddca9"
  "mid   ce30fe7  9eea289"
  "small 9eea289  0822cd8"
)

pass=0
fail=0

for row in "${CASES[@]}"; do
  read -r name parent commit <<<"$row"
  repo="$WORK/$name"

  git init -q -b main "$repo"
  git -C "$repo" config user.email smoke@example.com
  git -C "$repo" config user.name "Smoke Test"
  git -C "$SRC" archive "$parent" | tar -x -C "$repo"
  git -C "$repo" add -A >/dev/null
  git -C "$repo" commit -qm base
  git -C "$SRC" diff "$parent" "$commit" >"$WORK/$name.patch"
  git -C "$repo" apply "$WORK/$name.patch" 2>/dev/null
  git -C "$repo" add -A

  # stdout is captured (not a terminal), so it must carry exactly the final
  # cleaned message; live progress goes to stderr.
  out=$(cd "$repo" && "$ACM" --dry-run 2>"$WORK/$name.err")
  rc=$?

  first=$(printf '%s' "$out" | head -1)
  len=$(printf '%s' "$first" | wc -m)
  bullets=$(printf '%s\n' "$out" | tail -n +2 | grep -c '^- ')

  problems=()
  [ "$rc" -ne 0 ] && problems+=("exit=$rc")
  printf '%s' "$first" | grep -Eq \
    '^(fix|feat|refactor|docs|test|chore|style|perf|build|ci)(\([^()]+\))?!?: .+' \
    || problems+=("malformed header")
  [ "$len" -gt 72 ] && problems+=("subject ${len} chars > 72")

  while IFS= read -r line; do
    [ -z "$line" ] && continue
    case "$line" in
      "- "*) ;;
      *) problems+=("prose body: ${line:0:44}"); break ;;
    esac
  done < <(printf '%s\n' "$out" | tail -n +2)

  if [ ${#problems[@]} -eq 0 ]; then
    pass=$((pass + 1))
    printf '[PASS] %-6s %2s ch, %s bullets  %s\n' "$name" "$len" "$bullets" "$first"
  else
    fail=$((fail + 1))
    printf '[FAIL] %-6s %s\n' "$name" "$(IFS='; '; echo "${problems[*]}")"
    printf '       stderr: %s\n' "$(tail -2 "$WORK/$name.err" | tr '\n' ' ')"
  fi
done

echo
echo "model=$MODEL  pass=$pass  fail=$fail"
[ "$fail" -eq 0 ]
