#!/usr/bin/env bash

# ~/.config/dmux/service.env is a privileged write into the GUI session
# environment (ADR 012 WS-F.1), so its parser must accept exactly the grammar
# shared/wezterm/mux/dmux-service-env.sh documents and nothing hostile. These
# cases drive the shared parser under /bin/sh, the com.fredrir.dmux-env loader
# against a recording `launchctl`, and the mux wrapper's precedence against a
# recording `wezterm-mux-server`, all inside the sandbox HOME. Nothing here
# touches launchd, systemd, or the live runtime directory.

MUX_DIR="$SOURCE_ROOT/shared/wezterm/mux"

env_file() {
  printf '%s\n' "$XDG_CONFIG_HOME/dmux/service.env"
}

write_env() {
  mkdir -p "$XDG_CONFIG_HOME/dmux"
  printf '%s\n' "$@" > "$(env_file)"
}

# parse_env: run dmux_service_env_lines on the sandbox file under /bin/sh.
# OUTPUT is stdout only; ERR is stderr; STATUS the exit status.
parse_env() {
  OUTPUT="$(sh -c '. "$1/dmux-service-env.sh" && dmux_service_env_lines "$2"' \
    sh "$MUX_DIR" "$(env_file)" 2>"$SANDBOX/stderr")"
  STATUS=$?
  ERR="$(cat "$SANDBOX/stderr")"
  return 0
}

lookup_env() {
  OUTPUT="$(sh -c '. "$1/dmux-service-env.sh" || exit 9
    lines=$(dmux_service_env_lines "$2") || exit 9
    dmux_service_env_lookup "$3" "$lines"' sh "$MUX_DIR" "$(env_file)" "$1" 2>&1)"
  STATUS=$?
  return 0
}

assert_err_has() {
  case "$ERR" in
    *"$1"*) ;;
    *) fail "expected stderr to contain '$1':
$ERR" ;;
  esac
}

# Recording stand-ins for the privileged commands. `launchctl` and `logger`
# append their arguments to trace files instead of touching the host.
setup_shims() {
  SHIMS="$SANDBOX/bin"
  mkdir -p "$SHIMS"
  printf '%s\n' '#!/bin/sh' 'printf "%s\n" "$*" >> "$SANDBOX/launchctl.trace"' \
    > "$SHIMS/launchctl"
  printf '%s\n' '#!/bin/sh' 'printf "%s\n" "$*" >> "$SANDBOX/logger.trace"' \
    > "$SHIMS/logger"
  # A dmux that answers the three probes the wrapper makes on the managed
  # path, a pane-bootstrap beside it, and a mux server that reports what it
  # was handed instead of serving.
  printf '%s\n' '#!/bin/sh' \
    'case "$1" in' \
    '  _mux-idle) exit 0 ;;' \
    '  _bridge-key) exit 0 ;;' \
    '  _recovery) echo 0badcafe-0000-4000-8000-00000000f1f1 ;;' \
    '  *) exit 2 ;;' \
    'esac' > "$SHIMS/dmux"
  printf '%s\n' '#!/bin/sh' 'exit 0' > "$SHIMS/pane-bootstrap"
  printf '%s\n' '#!/bin/sh' \
    'printf "wez_first=%s\n" "${DMUX_WEZ_FIRST-unset}"' \
    'printf "legacy_policy=%s\n" "${DMUX_LEGACY_POLICY-unset}"' \
    'printf "backend_instance=%s\n" "${DMUX_BACKEND_INSTANCE-unset}"' \
    'printf "args=%s\n" "$*"' > "$SHIMS/wezterm-mux-server"
  chmod +x "$SHIMS"/*
  export SANDBOX
}

run_loader() {
  OUTPUT="$(PATH="$SHIMS:$PATH" sh "$MUX_DIR/dmux-env-load.sh" 2>&1)"
  STATUS=$?
  return 0
}

launchctl_trace() {
  [ -f "$SANDBOX/launchctl.trace" ] && cat "$SANDBOX/launchctl.trace"
  return 0
}

# run_wrapper [VAR=VALUE ...]: run dmux-mux-start.sh with the shims, the flag
# and opt-out scrubbed from the inherited environment, and only the given
# assignments set on the child. Never exports anything into this shell.
run_wrapper() {
  mkdir -p "$SANDBOX/run"
  OUTPUT="$(env -u DMUX_WEZ_FIRST -u DMUX_LEGACY_POLICY -u DMUX_BACKEND_INSTANCE \
    PATH="$SHIMS:$PATH" \
    XDG_RUNTIME_DIR="$SANDBOX/run" \
    DMUX_WEZTERM_MUX_SERVER="$SHIMS/wezterm-mux-server" \
    "$@" sh "$MUX_DIR/dmux-mux-start.sh" 2>&1)"
  STATUS=$?
  return 0
}

# The wrapper consults service.env on macOS only; Linux's one knob is
# environment.d and the file must be ignored there.
file_is_read_by_wrapper() {
  [ "$(uname -s)" = Darwin ]
}

test_parser_keeps_assignments_and_drops_blanks_and_comments() {
  write_env \
    '# policy for this host' \
    '' \
    'DMUX_WEZ_FIRST=1' \
    '   ' \
    '  # indented comment' \
    '  DMUX_LEGACY_POLICY=0' \
    'DMUX_WEZTERM_MUX_SERVER=/opt/homebrew/bin/wezterm-mux-server' \
    'DMUX_BACKEND_INSTANCE=0badcafe-0000-4000-8000-00000000f1f1' \
    'DMUX_=empty.key+is@allowed:by,the-grammar'
  parse_env
  assert_ok
  [ "$OUTPUT" = 'DMUX_WEZ_FIRST=1
DMUX_LEGACY_POLICY=0
DMUX_WEZTERM_MUX_SERVER=/opt/homebrew/bin/wezterm-mux-server
DMUX_BACKEND_INSTANCE=0badcafe-0000-4000-8000-00000000f1f1
DMUX_=empty.key+is@allowed:by,the-grammar' ] || fail "unexpected lines:
$OUTPUT"
  [ -z "$ERR" ] || fail "unexpected stderr: $ERR"
}

test_parser_accepts_an_absent_or_empty_file_as_no_preference() {
  parse_env
  assert_ok
  [ -z "$OUTPUT" ] || fail "absent file produced: $OUTPUT"
  write_env '# nothing stated' ''
  parse_env
  assert_ok
  [ -z "$OUTPUT" ] || fail "comment-only file produced: $OUTPUT"
}

test_parser_refuses_the_whole_file_on_a_wrong_key() {
  write_env \
    'DMUX_WEZ_FIRST=1' \
    'PATH=/tmp/evil' \
    'dmux_wez_first=1' \
    'DMUX_lower=1' \
    'DMUX_WEZ FIRST=1' \
    'DMUXWEZ=1' \
    'DMUX_WEZ_FIRST'
  parse_env
  assert_fails
  [ -z "$OUTPUT" ] || fail "a refused file must print nothing, got: $OUTPUT"
  assert_err_has "service.env:2: key must start with DMUX_"
  assert_err_has "service.env:3: key must start with DMUX_"
  assert_err_has "service.env:4: key must match"
  assert_err_has "service.env:5: key must match"
  assert_err_has "service.env:6: key must start with DMUX_"
  assert_err_has "service.env:7: expected KEY=VALUE"
  assert_err_has "refused; nothing applied"
  case "$ERR" in
    *evil*) fail "stderr must not echo line content: $ERR" ;;
  esac
}

test_parser_refuses_malicious_values_without_executing_them() {
  write_env \
    'DMUX_WEZ_FIRST=$(touch '"$SANDBOX"'/pwned)' \
    'DMUX_WEZ_FIRST=`touch '"$SANDBOX"'/pwned`' \
    'DMUX_WEZ_FIRST=1;touch '"$SANDBOX"'/pwned' \
    'DMUX_WEZ_FIRST=1 # trailing comment' \
    'DMUX_WEZ_FIRST="1"' \
    "DMUX_WEZ_FIRST='1'" \
    'DMUX_WEZ_FIRST=1 ' \
    'DMUX_WEZ_FIRST=a\nb' \
    'DMUX_WEZ_FIRST=~/x' \
    'DMUX_WEZ_FIRST=${HOME}' \
    'DMUX_WEZ_FIRST=1|cat' \
    'DMUX_WEZ_FIRST=1&' \
    'DMUX_WEZ_FIRST=>out'
  printf 'DMUX_WEZ_FIRST=1\r\n' >> "$(env_file)"
  parse_env
  assert_fails
  [ -z "$OUTPUT" ] || fail "a refused file must print nothing, got: $OUTPUT"
  local n
  for n in 1 2 3 4 5 6 7 8 9 10 11 12 13 14; do
    assert_err_has "service.env:$n: value must match"
  done
  assert_absent "$SANDBOX/pwned"
  case "$ERR" in
    *pwned*) fail "stderr must not echo line content: $ERR" ;;
  esac
}

test_parser_last_assignment_wins() {
  write_env 'DMUX_WEZ_FIRST=0' 'DMUX_LEGACY_POLICY=1' 'DMUX_WEZ_FIRST=1'
  lookup_env DMUX_WEZ_FIRST
  assert_ok
  [ "$OUTPUT" = 1 ] || fail "wanted 1, got '$OUTPUT'"
  lookup_env DMUX_LEGACY_POLICY
  assert_ok
  [ "$OUTPUT" = 1 ] || fail "wanted 1, got '$OUTPUT'"
  lookup_env DMUX_ABSENT
  assert_fails
  [ -z "$OUTPUT" ] || fail "absent key printed '$OUTPUT'"
}

test_loader_applies_each_line_in_order_through_launchctl_setenv() {
  setup_shims
  write_env '# canary' 'DMUX_WEZ_FIRST=0' 'DMUX_LEGACY_POLICY=1' 'DMUX_WEZ_FIRST=1'
  run_loader
  assert_ok
  [ "$(launchctl_trace)" = 'setenv DMUX_WEZ_FIRST 0
setenv DMUX_LEGACY_POLICY 1
setenv DMUX_WEZ_FIRST 1' ] || fail "launchctl trace:
$(launchctl_trace)"
  assert_output_has "applied 3 assignment(s)"
  assert_file_is "$SANDBOX/logger.trace" "-t dmux-env-load -- applied 3 assignment(s) from $(env_file) to the launchd session"
}

test_loader_refuses_a_malformed_file_and_calls_launchctl_for_nothing() {
  setup_shims
  write_env 'DMUX_WEZ_FIRST=1' 'DMUX_LEGACY_POLICY=$(touch '"$SANDBOX"'/pwned)'
  run_loader
  assert_fails
  assert_absent "$SANDBOX/launchctl.trace"
  assert_absent "$SANDBOX/pwned"
  assert_output_has "service.env:2: value must match"
  assert_output_has "refusing $(env_file): malformed; nothing applied"
}

test_loader_is_a_no_op_without_a_file() {
  setup_shims
  run_loader
  assert_ok
  assert_absent "$SANDBOX/launchctl.trace"
  assert_output_has "nothing to apply"
}

test_wrapper_process_environment_wins_over_the_file() {
  setup_shims
  write_env 'DMUX_WEZ_FIRST=1' 'DMUX_LEGACY_POLICY=1'
  run_wrapper DMUX_WEZ_FIRST=0 DMUX_LEGACY_POLICY=0
  assert_ok
  assert_output_has "wez_first=0"
  assert_output_has "legacy_policy=0"
  assert_output_has "backend_instance="
  assert_output_lacks "backend_instance=0badcafe"
}

test_wrapper_file_wins_over_the_tracked_default() {
  setup_shims
  write_env 'DMUX_WEZ_FIRST=1' 'DMUX_LEGACY_POLICY=1'
  run_wrapper
  assert_ok
  if file_is_read_by_wrapper; then
    assert_output_has "wez_first=1"
    assert_output_has "legacy_policy=1"
    assert_output_has "backend_instance=0badcafe-0000-4000-8000-00000000f1f1"
  else
    assert_output_has "wez_first=0"
    assert_output_has "legacy_policy=unset"
  fi
  assert_output_has "args=--dmux-managed-service --config-file $MUX_DIR/dmux-mux.lua"
}

test_wrapper_empty_process_value_states_nothing_so_the_file_applies() {
  setup_shims
  write_env 'DMUX_WEZ_FIRST=1'
  run_wrapper DMUX_WEZ_FIRST=
  assert_ok
  if file_is_read_by_wrapper; then
    assert_output_has "wez_first=1"
  else
    assert_output_has "wez_first=0"
  fi
}

test_wrapper_defaults_to_legacy_without_a_file() {
  setup_shims
  run_wrapper
  assert_ok
  assert_output_has "wez_first=0"
  assert_output_has "legacy_policy=unset"
  assert_output_has "backend_instance="
  assert_output_lacks "backend_instance=0badcafe"
}

test_wrapper_ignores_a_malformed_file_with_a_warning() {
  setup_shims
  write_env 'DMUX_WEZ_FIRST=1' 'DMUX_LEGACY_POLICY=`touch '"$SANDBOX"'/pwned`'
  run_wrapper
  assert_ok
  assert_output_has "wez_first=0"
  assert_output_has "legacy_policy=unset"
  assert_absent "$SANDBOX/pwned"
  if file_is_read_by_wrapper; then
    assert_output_has "WARN ignoring malformed $(env_file); tracked defaults apply"
  fi
}
