#!/usr/bin/env bash
set -uo pipefail

TESTS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
source "$TESTS_DIR/lib.sh"

BOLD=$'\033[1m'
RED=$'\033[31m'
GREEN=$'\033[32m'
DIM=$'\033[2m'
RESET=$'\033[0m'
[ -t 1 ] || { BOLD=""; RED=""; GREEN=""; DIM=""; RESET=""; }

passed=0
failed=0
failed_names=()

run_case_file() {
  local file="$1" name
  name="$(basename "${file%.sh}")"
  printf '%s%s%s\n' "$BOLD" "$name" "$RESET"

  local fn
  while IFS= read -r fn; do
    printf '  %-54s' "${fn#test_}"
    if ( set -uo pipefail; setup_sandbox; trap teardown_sandbox EXIT; source "$file"; "$fn" ) >"$TESTS_DIR/.out" 2>&1; then
      printf '%sok%s\n' "$GREEN" "$RESET"
      passed=$((passed + 1))
    else
      printf '%sFAIL%s\n' "$RED" "$RESET"
      failed=$((failed + 1))
      failed_names+=("$name/${fn#test_}")
      sed 's/^/      /' "$TESTS_DIR/.out" >&2
    fi
  done < <(grep -oE '^test_[A-Za-z0-9_]+' "$file")
}

if [ "$#" -gt 0 ]; then
  for pattern in "$@"; do
    for file in "$TESTS_DIR"/cases/*"$pattern"*.sh; do
      [ -f "$file" ] && run_case_file "$file"
    done
  done
else
  for file in "$TESTS_DIR"/cases/*.sh; do
    [ -f "$file" ] && run_case_file "$file"
  done
fi

rm -f "$TESTS_DIR/.out"

printf '\n%s%d passed, %d failed%s\n' "$BOLD" "$passed" "$failed" "$RESET"
if [ "$failed" -gt 0 ]; then
  printf '%s' "$DIM"
  printf '  %s\n' "${failed_names[@]}"
  printf '%s' "$RESET"
  exit 1
fi
