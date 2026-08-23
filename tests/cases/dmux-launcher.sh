#!/usr/bin/env bash

# macos/applications/dmux.app is the desktop entry point for WezTerm under
# dmux (ADR 012 §10, wave 4): under DMUX_WEZ_FIRST=1 the maintained fork
# refuses a bare Spotlight/Dock start of WezTerm, so the bundle's executable
# summons the resident GUI through `dmux _gui summon`; flag off it opens
# WezTerm itself. These cases drive that script with recording stand-ins for
# dmux, `open` and `osascript` inside the sandbox. Nothing here launches a
# GUI, touches launchd, or reaches the live runtime directory.

LAUNCHER="$SOURCE_ROOT/macos/applications/dmux.app/Contents/MacOS/dmux"

setup_launcher_fixtures() {
  STUBS="$SANDBOX/launcher-bin"
  TRACE="$SANDBOX/launcher.trace"
  mkdir -p "$STUBS"
  : > "$TRACE"
  printf '%s\n' '#!/bin/sh' 'printf "dmux %s\n" "$*" >> "$TRACE"' \
    'printf "%s\n" "${DMUX_STUB_OUTPUT:-{\"ok\":true}}"' 'exit "${DMUX_STUB_EXIT:-0}"' > "$STUBS/dmux"
  printf '%s\n' '#!/bin/sh' 'printf "open %s\n" "$*" >> "$TRACE"' > "$STUBS/open"
  printf '%s\n' '#!/bin/sh' 'printf "osascript %s\n" "$*" >> "$TRACE"' > "$STUBS/osascript"
  chmod +x "$STUBS/dmux" "$STUBS/open" "$STUBS/osascript"
  export TRACE
}

# run_launcher FLAG [ENV...]: run the bundle executable with the stand-ins
# wired through its seams and DMUX_WEZ_FIRST set to FLAG ("unset" removes it).
run_launcher() {
  flag="$1"; shift
  if [ "$flag" = unset ]; then
    OUTPUT="$(env -u DMUX_WEZ_FIRST "$@" DMUX_BIN="$STUBS/dmux" DMUX_OPEN_BIN="$STUBS/open" \
      DMUX_OSASCRIPT_BIN="$STUBS/osascript" sh "$LAUNCHER" 2>&1)"
  else
    OUTPUT="$(env DMUX_WEZ_FIRST="$flag" "$@" DMUX_BIN="$STUBS/dmux" DMUX_OPEN_BIN="$STUBS/open" \
      DMUX_OSASCRIPT_BIN="$STUBS/osascript" sh "$LAUNCHER" 2>&1)"
  fi
  STATUS=$?
  TRACED="$(cat "$TRACE")"
}

assert_trace_is() {
  [ "$TRACED" = "$1" ] || fail "trace mismatch
--- got ---
$TRACED
--- want ---
$1"
}

test_launcher_opens_plain_wezterm_when_the_flag_is_off() {
  setup_launcher_fixtures
  run_launcher unset
  assert_ok
  assert_trace_is 'open -b com.github.wez.wezterm'
  run_launcher 0
  assert_ok
  assert_trace_is 'open -b com.github.wez.wezterm
open -b com.github.wez.wezterm'
}

test_launcher_summons_through_the_broker_when_the_flag_is_on() {
  setup_launcher_fixtures
  run_launcher 1
  assert_ok
  # Exactly one dmux call, the broker path, and no dialog.
  assert_trace_is 'dmux _gui summon --format json'
}

test_launcher_shows_the_refusal_in_a_dialog_and_keeps_the_exit_status() {
  setup_launcher_fixtures
  run_launcher 1 DMUX_STUB_EXIT=6 DMUX_STUB_OUTPUT='{"ok":false,"error":"provider_unavailable","message":"managed Wez descriptor is absent"}'
  [ "$STATUS" -eq 6 ] || fail "expected the summon exit status 6, got $STATUS: $OUTPUT"
  case "$TRACED" in
    'dmux _gui summon --format json
osascript -e display dialog "dmux could not summon WezTerm (exit 6): '*'managed Wez descriptor is absent'*) ;;
    *) fail "expected a dialog naming the refusal:
$TRACED" ;;
  esac
}

test_launcher_explains_a_missing_dmux_instead_of_failing_silently() {
  setup_launcher_fixtures
  OUTPUT="$(env DMUX_WEZ_FIRST=1 DMUX_BIN="$SANDBOX/no-such-dmux" DMUX_OPEN_BIN="$STUBS/open" \
    DMUX_OSASCRIPT_BIN="$STUBS/osascript" sh "$LAUNCHER" 2>&1)"
  STATUS=$?
  TRACED="$(cat "$TRACE")"
  [ "$STATUS" -eq 1 ] || fail "expected exit 1, got $STATUS: $OUTPUT"
  case "$TRACED" in
    "osascript -e display dialog \"dmux is not installed at $SANDBOX/no-such-dmux; run 'dotfile sync'.\""*) ;;
    *) fail "expected an install dialog:
$TRACED" ;;
  esac
}
