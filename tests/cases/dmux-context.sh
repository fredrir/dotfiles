setup_dmux_context_fixtures() {
  command -v zsh >/dev/null 2>&1 || fail "zsh is required"
  command -v jq >/dev/null 2>&1 || fail "jq is required"
  REAL_JQ="$(command -v jq)"

  COMMANDS="$SANDBOX/dmux-context-bin"
  RESPONSE="$SANDBOX/dmux-context.json"
  TRACE="$SANDBOX/dmux-context.trace"
  OSC="$SANDBOX/dmux-context.osc"
  ERRORS="$SANDBOX/dmux-context.err"
  STATE="$SANDBOX/dmux-context.state"
  TMUX_TRACE="$SANDBOX/dmux-context.tmux.trace"
  JQ_TRACE="$SANDBOX/dmux-context.jq.trace"
  CONTEXT_HOOK="$SOURCE_ROOT/shared/zsh/conf.d/94-dmux-context.zsh"
  export RESPONSE TRACE OSC ERRORS STATE TMUX_TRACE JQ_TRACE REAL_JQ CONTEXT_HOOK
  mkdir -p "$COMMANDS"

  printf '%s\n' \
    '#!/bin/sh' \
    'printf "%s\n" "$*" >> "$TRACE"' \
    'if [ "${DMUX_TEST_REJECT_EXPORTED_HINT:-0}" = 1 ] \
      && /usr/bin/env | /usr/bin/grep -q "^_DMUX_CONTEXT_SPACE_UID_HINT="; then exit 8; fi' \
    '[ -z "${DMUX_TEST_SLEEP:-}" ] || exec /bin/sleep "$DMUX_TEST_SLEEP"' \
    '[ "${DMUX_TEST_FAIL:-0}" = 0 ] || exit 3' \
    '/bin/cat "$RESPONSE"' \
    > "$COMMANDS/dmux"
  chmod +x "$COMMANDS/dmux"

  printf '%s\n' \
    '#!/bin/sh' \
    'printf "%s\n" "$*" >> "$JQ_TRACE"' \
    '[ -z "${DMUX_TEST_JQ_SLEEP:-}" ] || exec /bin/sleep "$DMUX_TEST_JQ_SLEEP"' \
    'exec "$REAL_JQ" "$@"' \
    > "$COMMANDS/jq"
  chmod +x "$COMMANDS/jq"

  printf '%s\n' \
    '#!/bin/sh' \
    'printf "%s\n" "$*" >> "$TMUX_TRACE"' \
    '[ -z "${TMUX_TEST_SLEEP:-}" ] || exec /bin/sleep "$TMUX_TEST_SLEEP"' \
    'us=$(printf "\037")' \
    'target_session=${TMUX_TEST_TARGET_SESSION:-\$7}' \
    'target_window=${TMUX_TEST_TARGET_WINDOW:-@8}' \
    'target_pane=${TMUX_TEST_TARGET_PANE:-${TMUX_PANE:-%4}}' \
    '[ "$1" = display-message ] || exit 7' \
    '[ "${TMUX_TEST_DISPLAY_FAIL:-0}" = 0 ] || exit 4' \
    'printf "%s%s%s%s%s\n" "$target_session" "$us" "$target_window" "$us" "$target_pane"' \
    '[ "${TMUX_TEST_LIST_FAIL:-0}" = 0 ] || exit 5' \
    'client_session=${TMUX_TEST_CLIENT_SESSION:-\$7}' \
    'client_window=${TMUX_TEST_CLIENT_WINDOW:-@8}' \
    'client_pane=${TMUX_TEST_CLIENT_PANE:-%4}' \
    'case "${TMUX_TEST_CLIENT_MODE:-one}" in' \
    '  zero) ;;' \
    '  one) printf "%s%s%s%s%s\n" "$client_session" "$us" "$client_window" "$us" "$client_pane" ;;' \
    '  multi)' \
    '    printf "%s%s%s%s%s\n" "$client_session" "$us" "$client_window" "$us" "$client_pane"' \
    '    printf "%s%s%s%s%s\n" "\$9" "$us" "@10" "$us" "%11"' \
    '    ;;' \
    '  malformed) printf "not-a-client-row\n" ;;' \
    '  *) exit 6 ;;' \
    'esac' \
    > "$COMMANDS/tmux"
  chmod +x "$COMMANDS/tmux"
}

write_wez_context() {
  printf '%s\n' '{
  "host_uid": "11111111-1111-4111-8111-111111111111",
  "space_uid": "01890f47-6a3c-7cc0-8000-000000000001",
  "space_no": 7,
  "backend": "wez",
  "domain": "dmux_remote:route.one-2",
  "server_epoch": "22222222-2222-4222-8222-222222222222",
  "group_ref": "g22222222-2222-4222-8222-222222222222.wz-3",
  "split_ref": "p22222222-2222-4222-8222-222222222222.wz-4"
}' > "$RESPONSE"
}

write_tmux_context() {
  printf '%s\n' '{
  "host_uid": "11111111-1111-4111-8111-111111111111",
  "space_uid": "01890f47-6a3c-7cc0-8000-000000000001",
  "space_no": 7,
  "backend": "tmux",
  "domain": null,
  "server_epoch": "22222222-2222-4222-8222-222222222222",
  "group_ref": "g22222222-2222-4222-8222-222222222222.tx-3",
  "split_ref": "p22222222-2222-4222-8222-222222222222.tx-4"
}' > "$RESPONSE"
}

run_dmux_context_zsh() {
  PATH="$COMMANDS:$PATH" zsh -f -c "$1"
}

hex_file() {
  od -An -v -tx1 "$1" | tr -d ' \n'
}

osc_count() {
  local path="$1" prefix="${2:-1b5d313333373b536574557365725661723d}"
  local hex stripped
  hex="$(hex_file "$path")"
  stripped=${hex//$prefix/}
  printf '%s\n' "$(( (${#hex} - ${#stripped}) / ${#prefix} ))"
}

test_dmux_context_refresh_exports_validated_marker_and_coalesces_redraws() {
  setup_dmux_context_fixtures
  write_wez_context

  run_dmux_context_zsh '
    export DMUX_WEZ_FIRST=1 WEZTERM_PANE=17
    export DMUX_SPACE_UID=01890f47-6a3c-7cc0-8000-000000000001
    export DMUX_GROUP_REF=stale DMUX_SPLIT_REF=stale
    source $CONTEXT_HOOK
    _dmux_context_refresh > $OSC 2> $ERRORS
    _dmux_context_refresh >> $OSC 2>> $ERRORS
    {
      print -r -- "$DMUX_CONTEXT_VERSION|$DMUX_HOST_UID|$DMUX_SPACE_UID|$DMUX_SPACE_NO"
      print -r -- "$DMUX_BACKEND|$DMUX_DOMAIN|$DMUX_SERVER_EPOCH"
      print -r -- "$DMUX_GROUP_REF|$DMUX_SPLIT_REF"
      [[ ${(t)DMUX_SPLIT_REF} == *export* ]] && print exported
    } > $STATE
  ' || fail "valid prompt refresh failed"

  assert_file_is "$TRACE" '_context'
  assert_file_is "$ERRORS" ''
  assert_file_is "$STATE" '1|11111111-1111-4111-8111-111111111111|01890f47-6a3c-7cc0-8000-000000000001|7
wez|dmux_remote:route.one-2|22222222-2222-4222-8222-222222222222
g22222222-2222-4222-8222-222222222222.wz-3|p22222222-2222-4222-8222-222222222222.wz-4
exported'

  local hex count
  hex="$(hex_file "$OSC")"
  count="$(osc_count "$OSC")"
  [ "$count" -eq 20 ] || fail "two prompt emissions did not contain 20 ordinary OSC markers"
  case "$hex" in
    *646d75785f746d75785f636c69656e745f7569643d07*) ;;
    *) fail "validated direct-Wez context did not clear the prior tmux client UID" ;;
  esac
}

test_dmux_context_stale_failure_clears_environment_and_user_vars() {
  setup_dmux_context_fixtures
  write_wez_context

  run_dmux_context_zsh '
    export DMUX_WEZ_FIRST=1 WEZTERM_PANE=17 DMUX_TEST_FAIL=1
    export DMUX_CONTEXT_VERSION=1
    export DMUX_HOST_UID=11111111-1111-4111-8111-111111111111
    export DMUX_SPACE_UID=01890f47-6a3c-7cc0-8000-000000000001
    export DMUX_SPACE_NO=7 DMUX_BACKEND=wez DMUX_DOMAIN=local
    export DMUX_SERVER_EPOCH=22222222-2222-4222-8222-222222222222
    export DMUX_GROUP_REF=g22222222-2222-4222-8222-222222222222.wz-3
    export DMUX_SPLIT_REF=p22222222-2222-4222-8222-222222222222.wz-4
    source $CONTEXT_HOOK
    _dmux_context_refresh > $OSC 2> $ERRORS
    for name in DMUX_CONTEXT_VERSION DMUX_HOST_UID DMUX_SPACE_UID DMUX_SPACE_NO \
      DMUX_BACKEND DMUX_DOMAIN DMUX_SERVER_EPOCH DMUX_GROUP_REF DMUX_SPLIT_REF; do
      (( ${+parameters[$name]} == 0 )) || exit 20
    done
    print cleared > $STATE
  ' || fail "stale context did not fail closed"

  assert_file_is "$TRACE" '_context'
  assert_file_is "$STATE" 'cleared'
  assert_file_is "$ERRORS" 'dmux: pane context is invalid or stale; pane markers cleared'

  local hex clear_prefix stripped count
  hex="$(hex_file "$OSC")"
  clear_prefix=1b5d313333373b536574557365725661723d
  stripped=${hex//$clear_prefix/}
  count=$(( (${#hex} - ${#stripped}) / ${#clear_prefix} ))
  [ "$count" -eq 9 ] || fail "fail-closed path did not clear all nine Wez user variables"
  case "$hex" in
    *3d4d513d3d07*) fail "clear stream retained the base64 context version" ;;
  esac
}

test_dmux_context_tmux_uses_exact_passthrough_encoding() {
  setup_dmux_context_fixtures
  write_tmux_context

  run_dmux_context_zsh '
    export DMUX_WEZ_FIRST=1
    export TMUX=/tmp/tmux-1000/default,1,0 TMUX_PANE=%4
    export DMUX_SPACE_UID=01890f47-6a3c-7cc0-8000-000000000001
    source $CONTEXT_HOOK
    _dmux_context_refresh > $OSC 2> $ERRORS
  ' || fail "tmux prompt refresh failed"

  assert_file_is "$TRACE" '_context'
  assert_file_is "$ERRORS" ''

  local hex first prefix suffix without count
  hex="$(hex_file "$OSC")"
  first=1b50746d75783b1b1b5d313333373b536574557365725661723d646d75785f636f6e746578745f76657273696f6e3d4d513d3d071b5c
  case "$hex" in
    "$first"*) ;;
    *) fail "first marker does not match the frozen tmux DCS/OSC golden bytes" ;;
  esac
  prefix=1b50746d75783b1b1b5d
  without=${hex//$prefix/}
  count=$(( (${#hex} - ${#without}) / ${#prefix} ))
  [ "$count" -eq 9 ] || fail "expected nine tmux-wrapped marker prefixes, got $count"
  suffix=071b5c
  without=${hex//$suffix/}
  count=$(( (${#hex} - ${#without}) / ${#suffix} ))
  [ "$count" -eq 9 ] || fail "expected nine tmux ST terminators, got $count"
  case "$hex" in
    *646d75785f746d75785f636c69656e745f7569643d*)
      fail "validated tmux context unexpectedly cleared the tmux client UID"
      ;;
  esac
}

test_dmux_context_hostile_json_is_never_evaluated_or_emitted() {
  setup_dmux_context_fixtures
  local pwned="$SANDBOX/context-injection-ran"
  printf '{"host_uid":"11111111-1111-4111-8111-111111111111","space_uid":"01890f47-6a3c-7cc0-8000-000000000001","space_no":7,"backend":"wez","domain":"$(touch %s)","server_epoch":"22222222-2222-4222-8222-222222222222","group_ref":"g22222222-2222-4222-8222-222222222222.wz-3;print owned","split_ref":"p22222222-2222-4222-8222-222222222222.wz-4"}\n' \
    "$pwned" > "$RESPONSE"

  run_dmux_context_zsh '
    export DMUX_WEZ_FIRST=1 WEZTERM_PANE=17
    export DMUX_SPACE_UID=01890f47-6a3c-7cc0-8000-000000000001
    source $CONTEXT_HOOK
    _dmux_context_refresh > $OSC 2> $ERRORS
    (( ${+DMUX_SPACE_UID} == 0 )) || exit 21
  ' || fail "hostile context response did not fail closed"

  assert_absent "$pwned"
  assert_file_is "$ERRORS" 'dmux: pane context response is malformed; pane markers cleared'
  case "$(hex_file "$OSC")" in
    *24746f756368*|*3b7072696e74*) fail "hostile JSON field reached the OSC stream" ;;
  esac
}

test_dmux_context_missing_dmux_clears_without_a_prompt_storm() {
  setup_dmux_context_fixtures
  local empty_bin="$SANDBOX/empty-bin"
  mkdir -p "$empty_bin"

  EMPTY_BIN="$empty_bin" zsh -f -c '
    path=($EMPTY_BIN)
    rehash
    export DMUX_WEZ_FIRST=1 WEZTERM_PANE=17
    export DMUX_SPACE_UID=01890f47-6a3c-7cc0-8000-000000000001
    source $CONTEXT_HOOK
    _dmux_context_refresh > $OSC 2> $ERRORS
    _dmux_context_refresh >> $OSC 2>> $ERRORS
    (( ${+DMUX_SPACE_UID} == 0 )) || exit 22
  ' || fail "missing dmux path did not fail closed"

  assert_file_is "$ERRORS" 'dmux: dmux executable is unavailable; pane markers cleared'
  [ ! -s "$TRACE" ] || fail "missing-dmux test unexpectedly invoked a controller"
}

test_dmux_context_transient_failure_retries_from_private_unexported_locator() {
  setup_dmux_context_fixtures
  write_wez_context

  run_dmux_context_zsh '
    export DMUX_WEZ_FIRST=1 WEZTERM_PANE=17 DMUX_TEST_FAIL=1
    export DMUX_SPACE_UID=01890f47-6a3c-7cc0-8000-000000000001
    source $CONTEXT_HOOK
    _dmux_context_refresh > $OSC 2> $ERRORS
    (( ${+DMUX_SPACE_UID} == 0 )) || exit 30
    [[ $_DMUX_CONTEXT_SPACE_UID_HINT == 01890f47-6a3c-7cc0-8000-000000000001 ]] || exit 31
    [[ ${(t)_DMUX_CONTEXT_SPACE_UID_HINT} != *export* ]] || exit 32

    # The first retry is delayed, so a duplicate prompt neither calls dmux nor
    # restores an unvalidated public marker.
    DMUX_TEST_FAIL=0
    _dmux_context_refresh >> $OSC 2>> $ERRORS
    (( ${+DMUX_SPACE_UID} == 0 )) || exit 33

    # Force the deterministic test clock past the retry boundary. Only
    # `_context`, given the private locator for this one command, can restore.
    _DMUX_CONTEXT_RETRY_AFTER=0
    _dmux_context_refresh >> $OSC 2>> $ERRORS
    [[ $DMUX_SPACE_UID == $_DMUX_CONTEXT_SPACE_UID_HINT ]] || exit 34
    [[ $DMUX_BACKEND == wez ]] || exit 35
    print -r -- "$_DMUX_CONTEXT_RETRY_FAILURES|$_DMUX_CONTEXT_RETRY_AFTER" > $STATE
  ' || fail "transient context failure was not recoverable"

  assert_file_is "$TRACE" '_context
_context'
  assert_file_is "$ERRORS" 'dmux: pane context is invalid or stale; pane markers cleared'
  assert_file_is "$STATE" '0|0.0000000000'
  [ "$(osc_count "$OSC")" -eq 19 ] \
    || fail "retry path did not emit one nine-field clear and one ten-field Wez refresh"
}

test_dmux_context_empty_tmux_is_direct_wez_and_clears_client_uid() {
  setup_dmux_context_fixtures
  write_wez_context

  run_dmux_context_zsh '
    export DMUX_WEZ_FIRST=1 WEZTERM_PANE=17 TMUX=""
    export DMUX_SPACE_UID=01890f47-6a3c-7cc0-8000-000000000001
    source $CONTEXT_HOOK
    _dmux_context_refresh > $OSC 2> $ERRORS
  ' || fail "empty TMUX direct-Wez refresh failed"

  assert_file_is "$ERRORS" ''
  [ ! -s "$TMUX_TRACE" ] || fail "empty TMUX invoked the tmux ownership probe"
  [ "$(osc_count "$OSC")" -eq 10 ] || fail "empty TMUX did not emit ten ordinary OSC values"
  case "$(hex_file "$OSC")" in
    1b50746d75783b*) fail "empty TMUX incorrectly selected DCS passthrough" ;;
    *646d75785f746d75785f636c69656e745f7569643d07*) ;;
    *) fail "empty TMUX did not clear dmux_tmux_client_uid" ;;
  esac
}

test_dmux_context_backward_clock_never_extends_the_prompt_cache() {
  setup_dmux_context_fixtures
  write_wez_context

  run_dmux_context_zsh '
    export DMUX_WEZ_FIRST=1 WEZTERM_PANE=17
    export DMUX_SPACE_UID=01890f47-6a3c-7cc0-8000-000000000001
    source $CONTEXT_HOOK
    _dmux_context_refresh > $OSC 2> $ERRORS
    _DMUX_CONTEXT_REFRESHED_AT=$(( ${EPOCHREALTIME:-$SECONDS} + 10 ))
    _dmux_context_refresh >> $OSC 2>> $ERRORS
  ' || fail "backward-clock prompt refresh failed"

  assert_file_is "$TRACE" '_context
_context'
  assert_file_is "$ERRORS" ''
  [ "$(osc_count "$OSC")" -eq 20 ] || fail "backward clock suppressed a required refresh"
}

test_dmux_context_source_is_immune_to_existing_aliases() {
  setup_dmux_context_fixtures
  write_wez_context
  local injected="$SANDBOX/alias-injection"
  export injected

  run_dmux_context_zsh '
    export DMUX_WEZ_FIRST=1 WEZTERM_PANE=17
    export DMUX_SPACE_UID=01890f47-6a3c-7cc0-8000-000000000001
    alias always="BROKEN"
    alias export="print -r -- alias-export >> $injected"
    alias unset="print -r -- alias-unset >> $injected"
    source $CONTEXT_HOOK
    [[ ${options[aliases]} == on ]] || exit 40
    [[ $aliases[always] == BROKEN ]] || exit 41
    [[ $aliases[export] == "print -r -- alias-export >> $injected" ]] || exit 42
    [[ $aliases[unset] == "print -r -- alias-unset >> $injected" ]] || exit 43
    _dmux_context_refresh > $OSC 2> $ERRORS
    [[ $DMUX_BACKEND == wez ]] || exit 44
  ' || fail "source-time aliases rewrote the prompt hook"

  assert_absent "$injected"
  assert_file_is "$ERRORS" ''
  assert_file_is "$TRACE" '_context'
}

test_dmux_context_controller_call_has_an_end_to_end_deadline() {
  setup_dmux_context_fixtures
  write_wez_context

  run_dmux_context_zsh '
    export DMUX_WEZ_FIRST=1 WEZTERM_PANE=17 DMUX_TEST_SLEEP=4
    export DMUX_SPACE_UID=01890f47-6a3c-7cc0-8000-000000000001
    source $CONTEXT_HOOK
    _DMUX_CONTEXT_TIMEOUT_SECONDS=1
    start=${EPOCHREALTIME:-$SECONDS}
    _dmux_context_refresh > $OSC 2> $ERRORS
    elapsed=$(( ${EPOCHREALTIME:-$SECONDS} - start ))
    (( elapsed >= 0.7 && elapsed < 2.5 )) || exit 50
    (( ${+DMUX_SPACE_UID} == 0 )) || exit 51
    print -r -- $elapsed > $STATE
  ' || fail "hung _context was not bounded by the prompt deadline"

  assert_file_is "$TRACE" '_context'
  assert_file_is "$ERRORS" 'dmux: pane context is invalid or stale; pane markers cleared'
  [ "$(osc_count "$OSC")" -eq 9 ] || fail "timed-out context did not clear all public markers"
}

test_dmux_context_jq_validation_shares_the_end_to_end_deadline() {
  setup_dmux_context_fixtures
  write_wez_context

  run_dmux_context_zsh '
    export DMUX_WEZ_FIRST=1 WEZTERM_PANE=17 DMUX_TEST_JQ_SLEEP=4
    export DMUX_SPACE_UID=01890f47-6a3c-7cc0-8000-000000000001
    source $CONTEXT_HOOK
    _DMUX_CONTEXT_TIMEOUT_SECONDS=1
    start=${EPOCHREALTIME:-$SECONDS}
    _dmux_context_refresh > $OSC 2> $ERRORS
    elapsed=$(( ${EPOCHREALTIME:-$SECONDS} - start ))
    (( elapsed >= 0.7 && elapsed < 2.5 )) || exit 52
    (( ${+DMUX_SPACE_UID} == 0 )) || exit 53
  ' || fail "hung jq validation was not bounded by the prompt deadline"

  assert_file_is "$TRACE" '_context'
  assert_file_is "$ERRORS" 'dmux: pane context response is malformed; pane markers cleared'
  [ "$(osc_count "$OSC")" -eq 9 ] || fail "timed-out jq validation did not clear all markers"
}

test_dmux_context_tmux_gate_has_a_deadline() {
  setup_dmux_context_fixtures

  run_dmux_context_zsh '
    export DMUX_WEZ_FIRST=1 TMUX=/tmp/fake-tmux,1,0 TMUX_PANE=%4
    source $CONTEXT_HOOK
    _DMUX_CONTEXT_TIMEOUT_SECONDS=1
    start=${EPOCHREALTIME:-$SECONDS}
    TMUX_TEST_SLEEP=4 _dmux_context_emit a b c d e f g h i > $OSC
    elapsed=$(( ${EPOCHREALTIME:-$SECONDS} - start ))
    (( elapsed >= 0.7 && elapsed < 2.5 )) || exit 54
  ' || fail "hung tmux ownership probe was not bounded"

  [ ! -s "$OSC" ] || fail "timed-out tmux ownership probe emitted passthrough"
  local us expected
  us="$(printf '\037')"
  expected="display-message -p -t %4 #{session_id}${us}#{window_id}${us}#{pane_id} ; list-clients -F #{session_id}${us}#{window_id}${us}#{pane_id}"
  assert_file_is "$TMUX_TRACE" "$expected"
}

test_dmux_context_inherited_private_locator_is_forcibly_unexported() {
  setup_dmux_context_fixtures
  write_wez_context

  run_dmux_context_zsh '
    export DMUX_WEZ_FIRST=1 WEZTERM_PANE=17 DMUX_TEST_REJECT_EXPORTED_HINT=1
    export _DMUX_CONTEXT_SPACE_UID_HINT=01890f47-6a3c-7cc0-8000-000000000001
    source $CONTEXT_HOOK
    [[ ${(t)_DMUX_CONTEXT_SPACE_UID_HINT} != *export* ]] || exit 55
    /usr/bin/env | /usr/bin/grep -q "^_DMUX_CONTEXT_SPACE_UID_HINT=" && exit 56
    _dmux_context_refresh > $OSC 2> $ERRORS
    [[ $DMUX_SPACE_UID == 01890f47-6a3c-7cc0-8000-000000000001 ]] || exit 58
  ' || fail "inherited private locator retained its export attribute"

  assert_file_is "$TRACE" '_context'
  assert_file_is "$ERRORS" ''
}

test_dmux_context_rejects_valid_document_for_a_different_space() {
  setup_dmux_context_fixtures
  write_wez_context

  run_dmux_context_zsh '
    export DMUX_WEZ_FIRST=1 WEZTERM_PANE=17
    export DMUX_SPACE_UID=01890f47-6a3c-7cc0-8000-000000000099
    source $CONTEXT_HOOK
    _dmux_context_refresh > $OSC 2> $ERRORS
    (( ${+DMUX_SPACE_UID} == 0 )) || exit 59
  ' || fail "valid response for a different SpaceUid did not fail closed"

  assert_file_is "$ERRORS" 'dmux: pane context response is malformed; pane markers cleared'
  [ "$(osc_count "$OSC")" -eq 9 ] || fail "wrong-Space response did not clear public markers"
}

test_dmux_context_oversize_check_preserves_trailing_newlines() {
  setup_dmux_context_fixtures
  # 8192 JSON whitespace bytes plus a newline used to shrink below the bound
  # when zsh command substitution stripped trailing newlines.
  /usr/bin/printf '%8192s\n' '' > "$RESPONSE"

  run_dmux_context_zsh '
    export DMUX_WEZ_FIRST=1 WEZTERM_PANE=17
    export DMUX_SPACE_UID=01890f47-6a3c-7cc0-8000-000000000001
    source $CONTEXT_HOOK
    _dmux_context_refresh > $OSC 2> $ERRORS
    (( ${+DMUX_SPACE_UID} == 0 )) || exit 60
  ' || fail "8193-byte newline-terminated response bypassed the size limit"

  assert_file_is "$ERRORS" 'dmux: pane context response is oversized; pane markers cleared'
}

test_dmux_context_fake_tmux_gate_requires_one_exact_active_client() {
  setup_dmux_context_fixtures

  run_dmux_context_zsh '
    export DMUX_WEZ_FIRST=1 TMUX=/tmp/fake-tmux,1,0 TMUX_PANE=%4
    source $CONTEXT_HOOK
    marker=(M Q U X QkE= R A S E)

    TMUX_TEST_CLIENT_MODE=zero _dmux_context_emit $marker > $OSC.zero
    TMUX_TEST_CLIENT_MODE=multi _dmux_context_emit $marker > $OSC.multi
    TMUX_TEST_CLIENT_MODE=malformed _dmux_context_emit $marker > $OSC.malformed
    TMUX_TEST_CLIENT_MODE=one TMUX_TEST_CLIENT_SESSION=\$9 \
      _dmux_context_emit $marker > $OSC.session
    TMUX_TEST_CLIENT_MODE=one TMUX_TEST_CLIENT_WINDOW=@9 \
      _dmux_context_emit $marker > $OSC.window
    TMUX_TEST_CLIENT_MODE=one TMUX_TEST_CLIENT_PANE=%9 \
      _dmux_context_emit $marker > $OSC.hidden
    TMUX_TEST_CLIENT_MODE=one TMUX_TEST_CLIENT_PANE=%4 \
      _dmux_context_emit $marker > $OSC
  ' || fail "fake tmux client ownership gate failed"

  [ ! -s "$OSC.zero" ] || fail "zero-client tmux emitted passthrough"
  [ ! -s "$OSC.multi" ] || fail "multi-client tmux emitted passthrough"
  [ ! -s "$OSC.malformed" ] || fail "malformed client inventory emitted passthrough"
  [ ! -s "$OSC.session" ] || fail "wrong-session tmux client emitted passthrough"
  [ ! -s "$OSC.window" ] || fail "wrong-window tmux client emitted passthrough"
  [ ! -s "$OSC.hidden" ] || fail "inactive tmux pane emitted passthrough"
  [ "$(osc_count "$OSC" 1b50746d75783b1b1b5d)" -eq 9 ] \
    || fail "single exact active tmux client did not receive nine passthrough values"

  local us expected line
  us="$(printf '\037')"
  expected="display-message -p -t %4 #{session_id}${us}#{window_id}${us}#{pane_id} ; list-clients -F #{session_id}${us}#{window_id}${us}#{pane_id}"
  while IFS= read -r line; do
    [ "$line" = "$expected" ] || fail "tmux ownership probe argv was not the frozen complete-server query"
  done < "$TMUX_TRACE"
}

test_dmux_context_real_tmux_gate_suppresses_zero_multiple_and_hidden_clients() (
  setup_dmux_context_fixtures
  command -v tmux >/dev/null 2>&1 || fail "tmux is required"

  local namespace="dmux-context-test-$$-$RANDOM"
  local fifo_one="$SANDBOX/tmux-control-one" fifo_two="$SANDBOX/tmux-control-two"
  local control_one="$SANDBOX/tmux-control-one.out" control_two="$SANDBOX/tmux-control-two.out"
  local client_one= client_two=
  cleanup_dmux_context_tmux() {
    [ -z "$client_one" ] || kill "$client_one" 2>/dev/null || true
    [ -z "$client_two" ] || kill "$client_two" 2>/dev/null || true
    tmux -L "$namespace" kill-server 2>/dev/null || true
    [ -z "$client_one" ] || wait "$client_one" 2>/dev/null || true
    [ -z "$client_two" ] || wait "$client_two" 2>/dev/null || true
  }
  trap cleanup_dmux_context_tmux EXIT INT TERM

  tmux -L "$namespace" new-session -d -s dmux-context '/bin/sleep 120' \
    || fail "could not create scratch tmux server"
  local pane_one pane_two socket_path tmux_env
  pane_one="$(tmux -L "$namespace" display-message -p -t dmux-context: '#{pane_id}')"
  pane_two="$(tmux -L "$namespace" split-window -d -P -F '#{pane_id}' -t "$pane_one" '/bin/sleep 120')"
  tmux -L "$namespace" select-pane -t "$pane_one"
  socket_path="$(tmux -L "$namespace" display-message -p -t "$pane_one" '#{socket_path}')"
  tmux_env="$socket_path,1,0"

  DMUX_WEZ_FIRST=1 TMUX="$tmux_env" TMUX_PANE="$pane_one" \
    zsh -f -c 'source $CONTEXT_HOOK; _dmux_context_emit a b c d e f g h i' \
    > "$OSC.zero" || fail "zero-client real tmux probe failed"
  [ ! -s "$OSC.zero" ] || fail "real zero-client tmux emitted passthrough"

  mkfifo "$fifo_one"
  exec 9<> "$fifo_one"
  tmux -L "$namespace" -C attach-session -t dmux-context \
    <&9 > "$control_one" 2>&1 &
  client_one=$!
  local attempts clients=0
  attempts=0
  while [ "$attempts" -lt 250 ]; do
    attempts=$((attempts + 1))
    clients="$(tmux -L "$namespace" list-clients -F '#{client_pid}' 2>/dev/null | wc -l | tr -d ' ')"
    [ "$clients" -eq 1 ] && break
    /bin/sleep 0.02
  done
  [ "$clients" -eq 1 ] || fail "scratch tmux control client did not attach"

  tmux -L "$namespace" select-pane -t "$pane_one"
  DMUX_WEZ_FIRST=1 TMUX="$tmux_env" TMUX_PANE="$pane_one" \
    zsh -f -c 'source $CONTEXT_HOOK; _dmux_context_emit a b c d e f g h i' \
    > "$OSC" || fail "active real tmux probe failed"
  [ "$(osc_count "$OSC" 1b50746d75783b1b1b5d)" -eq 9 ] \
    || fail "one exact active real tmux client did not receive passthrough"

  DMUX_WEZ_FIRST=1 TMUX="$tmux_env" TMUX_PANE="$pane_two" \
    zsh -f -c 'source $CONTEXT_HOOK; _dmux_context_emit a b c d e f g h i' \
    > "$OSC.hidden" || fail "hidden real tmux probe failed"
  [ ! -s "$OSC.hidden" ] || fail "real hidden tmux pane emitted passthrough"

  mkfifo "$fifo_two"
  # Keep below fd 10: Bash 3 may reserve fd 10 while the harness iterates its
  # process-substitution test list.
  exec 8<> "$fifo_two"
  tmux -L "$namespace" -C attach-session -t dmux-context \
    <&8 > "$control_two" 2>&1 &
  client_two=$!
  attempts=0
  while [ "$attempts" -lt 250 ]; do
    attempts=$((attempts + 1))
    clients="$(tmux -L "$namespace" list-clients -F '#{client_pid}' 2>/dev/null | wc -l | tr -d ' ')"
    [ "$clients" -eq 2 ] && break
    /bin/sleep 0.02
  done
  [ "$clients" -eq 2 ] || fail "second scratch tmux control client did not attach"

  DMUX_WEZ_FIRST=1 TMUX="$tmux_env" TMUX_PANE="$pane_one" \
    zsh -f -c 'source $CONTEXT_HOOK; _dmux_context_emit a b c d e f g h i' \
    > "$OSC.multi" || fail "multiple-client real tmux probe failed"
  [ ! -s "$OSC.multi" ] || fail "real multi-client tmux emitted passthrough"
)

test_dmux_context_flag_off_is_inert() {
  setup_dmux_context_fixtures

  # Flag off is the explicit opt-out DMUX_WEZ_FIRST=0 (ADR 010 §5); unset
  # states no preference and means Wez-first since the §21 step 9 flip.
  run_dmux_context_zsh '
    export DMUX_WEZ_FIRST=0
    export DMUX_GROUP_REF=legacy-marker
    source $CONTEXT_HOOK > $OSC 2> $ERRORS
    [[ $DMUX_GROUP_REF == legacy-marker ]] || exit 23
    (( ${precmd_functions[(Ie)_dmux_context_refresh]:-0} == 0 )) || exit 24
  ' || fail "flag-off prompt hook was not inert"

  [ ! -s "$TRACE" ] || fail "flag-off hook invoked dmux"
  [ ! -s "$OSC" ] || fail "flag-off hook emitted terminal control sequences"
  assert_file_is "$ERRORS" ''
}

# ---------------------------------------------------------------------------
# ADR 012 WS-E.2, site 94-dmux-context.zsh:216 (report 08 §7). The hook is
# the carrier of `server_epoch` from `dmux _context` to the pane's user
# variables. It verifies nothing about the epoch itself — plan §13.1 puts
# that in the crate, which now resolves the Space's instance from the
# registry and pins the scan to the published epoch (WS-A.7,
# tests/context_cli.rs) — so the property the hook must hold is narrower:
# every epoch it exports or emits comes from one validated response for the
# requested Space, child refs are refused unless they carry that same
# epoch, the controller is handed the bare `_context` argv and the Space
# locator and nothing that could steer its verification, and a controller
# refusal retires the previous epoch from the environment and the pane.

test_dmux_context_rejects_child_refs_outside_the_reported_epoch() {
  setup_dmux_context_fixtures
  # A response whose Group ref carries an epoch other than its own
  # `server_epoch`: a marker stitched from two incarnations.
  printf '%s\n' '{
  "host_uid": "11111111-1111-4111-8111-111111111111",
  "space_uid": "01890f47-6a3c-7cc0-8000-000000000001",
  "space_no": 7,
  "backend": "wez",
  "domain": null,
  "server_epoch": "22222222-2222-4222-8222-222222222222",
  "group_ref": "g33333333-3333-4333-8333-333333333333.wz-3",
  "split_ref": "p22222222-2222-4222-8222-222222222222.wz-4"
}' > "$RESPONSE"

  run_dmux_context_zsh '
    export DMUX_WEZ_FIRST=1 WEZTERM_PANE=17
    export DMUX_SPACE_UID=01890f47-6a3c-7cc0-8000-000000000001
    source $CONTEXT_HOOK
    _dmux_context_refresh > $OSC 2> $ERRORS
    (( ${+DMUX_SERVER_EPOCH} == 0 && ${+DMUX_GROUP_REF} == 0 )) || exit 61
  ' || fail "a child ref from another epoch was exported"

  assert_file_is "$TRACE" '_context'
  assert_file_is "$ERRORS" 'dmux: pane context response is malformed; pane markers cleared'
  [ "$(osc_count "$OSC")" -eq 9 ] || fail "mixed-epoch response did not clear public markers"
  case "$(hex_file "$OSC")" in
    *33333333*) fail "the foreign epoch reached the pane" ;;
  esac

  # The same with the Split ref, and with a provider that is not the
  # backend's (a tmux handle under a Wez marker).
  local variant
  for variant in \
    '"group_ref": "g22222222-2222-4222-8222-222222222222.wz-3", "split_ref": "p33333333-3333-4333-8333-333333333333.wz-4"' \
    '"group_ref": "g22222222-2222-4222-8222-222222222222.tx-3", "split_ref": "p22222222-2222-4222-8222-222222222222.wz-4"'; do
    printf '{"host_uid":"11111111-1111-4111-8111-111111111111","space_uid":"01890f47-6a3c-7cc0-8000-000000000001","space_no":7,"backend":"wez","domain":null,"server_epoch":"22222222-2222-4222-8222-222222222222",%s}\n' \
      "$variant" > "$RESPONSE"
    : > "$OSC"; : > "$ERRORS"
    run_dmux_context_zsh '
      export DMUX_WEZ_FIRST=1 WEZTERM_PANE=17
      export DMUX_SPACE_UID=01890f47-6a3c-7cc0-8000-000000000001
      source $CONTEXT_HOOK
      _dmux_context_refresh > $OSC 2> $ERRORS
      (( ${+DMUX_SERVER_EPOCH} == 0 )) || exit 62
    ' || fail "variant was exported: $variant"
    assert_file_is "$ERRORS" 'dmux: pane context response is malformed; pane markers cleared'
    [ "$(osc_count "$OSC")" -eq 9 ] || fail "variant did not clear public markers: $variant"
  done
}

test_dmux_context_epoch_rotation_is_taken_only_from_the_validated_response() {
  setup_dmux_context_fixtures
  write_wez_context

  # The pane still carries the previous incarnation's marker; the controller
  # answers for the replacement (22222222…). Nothing of the old epoch may
  # survive in the environment or on the wire, and the controller receives
  # the bare `_context` argv — no epoch, socket, namespace or seam argument
  # through which the hook could steer what the crate verifies.
  run_dmux_context_zsh '
    export DMUX_WEZ_FIRST=1 WEZTERM_PANE=17
    export DMUX_CONTEXT_VERSION=1
    export DMUX_HOST_UID=11111111-1111-4111-8111-111111111111
    export DMUX_SPACE_UID=01890f47-6a3c-7cc0-8000-000000000001
    export DMUX_SPACE_NO=7 DMUX_BACKEND=wez DMUX_DOMAIN=dmux_remote:route.one-2
    export DMUX_SERVER_EPOCH=33333333-3333-4333-8333-333333333333
    export DMUX_GROUP_REF=g33333333-3333-4333-8333-333333333333.wz-3
    export DMUX_SPLIT_REF=p33333333-3333-4333-8333-333333333333.wz-4
    source $CONTEXT_HOOK
    _dmux_context_refresh > $OSC 2> $ERRORS
    {
      print -r -- "$DMUX_SERVER_EPOCH"
      print -r -- "$DMUX_GROUP_REF|$DMUX_SPLIT_REF"
    } > $STATE
  ' || fail "epoch rotation refresh failed"

  assert_file_is "$TRACE" '_context'
  assert_file_is "$ERRORS" ''
  assert_file_is "$STATE" '22222222-2222-4222-8222-222222222222
g22222222-2222-4222-8222-222222222222.wz-3|p22222222-2222-4222-8222-222222222222.wz-4'
  [ "$(osc_count "$OSC")" -eq 10 ] || fail "rotation did not emit one ten-field Wez refresh"
  local hex
  hex="$(hex_file "$OSC")"
  # base64("22222222-2222-4222-8222-222222222222") begins MjIyMjIyMjIt;
  # base64("33333333-…") begins MzMzMzMzMzMt. Only the new one may appear.
  case "$hex" in
    *4d6a49794d6a49794d6a4974*) ;;
    *) fail "the validated epoch did not reach the pane" ;;
  esac
  case "$hex" in
    *4d7a4d7a4d7a4d7a4d7a4d74*) fail "the retired epoch was re-emitted to the pane" ;;
  esac
}

test_dmux_context_controller_refusal_retires_the_prior_epoch_from_env_and_pane() {
  setup_dmux_context_fixtures
  write_wez_context

  # The fixed crate exits non-zero with no document for a stranger endpoint,
  # a NULL published epoch, a rebound Space and a replaced server
  # (tests/context_cli.rs). The hook must treat every such refusal as the
  # end of the marker it was carrying, not keep the last good epoch.
  run_dmux_context_zsh '
    export DMUX_WEZ_FIRST=1 WEZTERM_PANE=17 DMUX_TEST_FAIL=1
    export DMUX_CONTEXT_VERSION=1
    export DMUX_HOST_UID=11111111-1111-4111-8111-111111111111
    export DMUX_SPACE_UID=01890f47-6a3c-7cc0-8000-000000000001
    export DMUX_SPACE_NO=7 DMUX_BACKEND=wez DMUX_DOMAIN=dmux_remote:route.one-2
    export DMUX_SERVER_EPOCH=22222222-2222-4222-8222-222222222222
    export DMUX_GROUP_REF=g22222222-2222-4222-8222-222222222222.wz-3
    export DMUX_SPLIT_REF=p22222222-2222-4222-8222-222222222222.wz-4
    source $CONTEXT_HOOK
    _dmux_context_refresh > $OSC 2> $ERRORS
    (( ${+DMUX_SERVER_EPOCH} == 0 && ${+DMUX_GROUP_REF} == 0 && ${+DMUX_SPLIT_REF} == 0 )) || exit 63
    # A duplicate prompt inside the retry window re-emits nothing: the
    # cache is empty, and the controller is not asked again.
    _dmux_context_refresh >> $OSC 2>> $ERRORS
    (( ${+DMUX_SERVER_EPOCH} == 0 )) || exit 64
  ' || fail "a controller refusal left the previous epoch in place"

  assert_file_is "$TRACE" '_context'
  assert_file_is "$ERRORS" 'dmux: pane context is invalid or stale; pane markers cleared'
  [ "$(osc_count "$OSC")" -eq 9 ] || fail "refusal did not emit exactly one nine-field clear"
  case "$(hex_file "$OSC")" in
    *4d6a49794d6a49794d6a4974*|*32323232323232322d*) fail "the retired epoch was emitted after the refusal" ;;
  esac
}
