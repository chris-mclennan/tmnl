#!/usr/bin/env bash
# PostToolUse hook: if the edited file was a Rust source, run cargo fmt
# on just that file so the project stays formatted without reformatting
# the world on every edit.
#
# Hook input arrives on stdin as JSON:
#   { "tool_input": { "file_path": "..." }, ... }
set -euo pipefail

input=$(cat)
path=$(printf '%s' "$input" | /usr/bin/python3 -c \
  'import json,sys; d=json.load(sys.stdin); print(d.get("tool_input",{}).get("file_path",""))' \
  2>/dev/null || true)

case "$path" in
  *.rs)
    cd "${CLAUDE_PROJECT_DIR:-.}"
    cargo fmt -- "$path" >/dev/null 2>&1 || true
    ;;
esac

exit 0
