#!/bin/sh
# One entry point for the dmux GUI-side test suite.
#
# Every test file is listed here with the exact preconditions it needs. Nothing
# is discovered and run blind, and an unlisted .lua file is a failure rather
# than a silent skip: a test nobody runs is how the leaked connection-UI domain
# survived a suite that already covered rogue domains.
#
# The preconditions are not uniform, which is why running the directory in a
# loop never worked:
#
#   - DMUX_WEZ_FIRST is exported by a managed GUI's own shell, so the flag-off
#     tests need it removed rather than merely not set. Scrub it everywhere and
#     hand it back only where it is wanted.
#   - top_level_missing_descriptor writes a descriptor into DMUX_RUNTIME_DIR
#     after asserting its absence, so every test gets its own directory.
#   - hammerspoon branches on three variables and covers an eighth of itself
#     per invocation.
#   - show_keys_config.lua is a config fixture injected into a real wezterm-gui
#     by managed_show_keys.sh, not a standalone test.
set -eu

tests_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH= cd -- "$tests_dir/../../../../.." && pwd)
cd "$repo_root"

lua_bin=${DMUX_TEST_LUA:-lua}
command -v "$lua_bin" >/dev/null 2>&1 || {
  echo "suite requires a lua interpreter; set DMUX_TEST_LUA to override" >&2
  exit 1
}

passed=0
failed=0
skipped=0
failures=''

report() {
  case $1 in
  ok)
    passed=$((passed + 1))
    printf '  %-56s ok\n' "$2"
    ;;
  fail)
    failed=$((failed + 1))
    failures="$failures$2
"
    printf '  %-56s FAIL\n' "$2"
    ;;
  skip)
    skipped=$((skipped + 1))
    printf '  %-56s SKIP (%s)\n' "$2" "$3"
    ;;
  esac
}

run() {
  label=$1
  shift
  if output=$("$@" 2>&1); then
    report ok "$label"
  else
    report fail "$label"
    printf '%s\n' "$output" | sed 's/^/      /' >&2
  fi
}

# Ambient dmux state must never decide a result. Each mode re-adds only what it
# needs on top of a scrubbed environment.
scrubbed() {
  env -u DMUX_WEZ_FIRST -u DMUX_RUNTIME_DIR -u HAMMER_APP_STATE -u HAMMER_FRONTMOST "$@"
}

run_unit() {
  run "$1" scrubbed "$lua_bin" "$tests_dir/$1.lua"
}

run_managed() {
  runtime=$(mktemp -d)
  run "$1" scrubbed "DMUX_WEZ_FIRST=1" "DMUX_RUNTIME_DIR=$runtime" "$lua_bin" "$tests_dir/$1.lua"
  rm -rf "$runtime"
}

run_hammerspoon() {
  state=$1
  managed=$2
  frontmost=$3
  set -- scrubbed "HAMMER_APP_STATE=$state"
  [ "$managed" = 0 ] || set -- "$@" DMUX_WEZ_FIRST=1
  [ "$frontmost" = 0 ] || set -- "$@" HAMMER_FRONTMOST=1
  run "hammerspoon [state=$state managed=$managed frontmost=$frontmost]" \
    "$@" "$lua_bin" "$tests_dir/hammerspoon.lua"
}

# unit      no dmux environment at all
# managed   DMUX_WEZ_FIRST=1 and a private DMUX_RUNTIME_DIR
# flag-off  asserts the managed flag is absent
# matrix    parameterised, run once per combination
# fixture   not a standalone test
mode_for() {
  case $1 in
  actions | consumer | controller | instance | mux_startup_witness | presentation | run) echo unit ;;
  config | config_linux | domains | picker | remote | resident_ingress | status) echo managed ;;
  top_level | top_level_missing_descriptor | top_level_missing_key) echo managed ;;
  config_off | top_level_off) echo flag-off ;;
  hammerspoon) echo matrix ;;
  show_keys_config) echo fixture ;;
  *) echo unknown ;;
  esac
}

echo 'dmux GUI test suite'
for file in "$tests_dir"/*.lua; do
  name=$(basename "$file" .lua)
  case $(mode_for "$name") in
  unit | flag-off) run_unit "$name" ;;
  managed) run_managed "$name" ;;
  matrix) : ;;
  fixture) report skip "$name" 'config fixture for managed_show_keys.sh' ;;
  unknown)
    report fail "$name"
    echo "      no preconditions declared for this test; add it to mode_for in suite.sh" >&2
    ;;
  esac
done

for state in absent zero; do
  for managed in 1 0; do
    for frontmost in 1 0; do
      run_hammerspoon "$state" "$managed" "$frontmost"
    done
  done
done

# These drive a real maintained-fork checkout or binary. They are skipped when
# it is absent, but never quietly: the summary carries the count.
if [ -n "${DMUX_WEZTERM_SOURCE:-}" ]; then
  run 'fork_surface.sh' sh "$tests_dir/fork_surface.sh"
else
  report skip 'fork_surface.sh' 'set DMUX_WEZTERM_SOURCE'
fi
if [ -n "${DMUX_WEZTERM_BIN:-}" ]; then
  run 'managed_show_keys.sh' sh "$tests_dir/managed_show_keys.sh"
else
  report skip 'managed_show_keys.sh' 'set DMUX_WEZTERM_BIN'
fi

printf '\n%d passed, %d failed, %d skipped\n' "$passed" "$failed" "$skipped"
if [ "$failed" -gt 0 ]; then
  printf '%s' "$failures" | sed 's/^/  /' >&2
  exit 1
fi
