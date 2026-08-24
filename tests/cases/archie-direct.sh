#!/usr/bin/env bash

controller="$SOURCE_ROOT/macos/zsh/conf.d/92-archie-direct.zsh"
host_helper="$SOURCE_ROOT/linux/arch/archie-direct/usr/local/libexec/archie-direct-host"

test_start_does_not_assign_zshs_read_only_status_parameter() {
  command -v zsh >/dev/null 2>&1 || return 0

  local output result
  output="$(zsh -f -c "
    source '$controller'
    _archie_direct_profile_saved() { return 0 }
    _archie_direct_remote_start() { return 0 }
    _archie_direct_join() { return 0 }
    _archie_direct_status() { return 0 }
    ssh() {
      [[ \"\$*\" == *'status json'* ]] && print -r -- '{\"active\":true,\"mode\":\"shared\"}'
      return 0
    }
    jq() { cat >/dev/null }
    _archie_direct_start shared
  " 2>&1)"
  result=$?

  [ "$result" -eq 0 ] || fail "mocked shared start failed:\n$output"
  case "$output" in
    *'read-only variable'*) fail "shared start assigned zsh's read-only status parameter" ;;
  esac
}

test_join_has_one_bounded_retry_budget() {
  command -v zsh >/dev/null 2>&1 || return 0

  local output sleeps network_calls
  output="$(zsh -f -c "
    source '$controller'
    integer network_calls=0 sleep_calls=0
    networksetup() { (( network_calls++ )); return 0 }
    ipconfig() { return 1 }
    sleep() { (( sleep_calls++ )); return 0 }
    _archie_direct_join >/dev/null 2>&1
    print -r -- \"\$sleep_calls \$network_calls\"
  ")"
  read -r sleeps network_calls <<< "$output"

  [ "$sleeps" -le 45 ] || fail "a failed join consumed $sleeps seconds of retry budget"
  [ "$network_calls" -le 10 ] || fail "a failed join attempted association $network_calls times"
}

test_enrollment_waits_for_the_address_after_the_manual_join() {
  command -v zsh >/dev/null 2>&1 || return 0
  mkdir -p "$HOME/dotfiles"
  : > "$HOME/dotfiles/vars.enc.yaml"

  local output result
  output="$(printf '\n' | zsh -f -c "
    source '$controller'
    integer sleep_calls=0
    _archie_direct_remote_start() { return 0 }
    sops() { print -r -- '\"mock-password\"' }
    pbpaste() { print -r -- old }
    pbcopy() { cat >/dev/null }
    open() { return 0 }
    ipconfig() { (( sleep_calls >= 1 )) && print -r -- 10.77.78.1 }
    sleep() { (( sleep_calls++ )); return 0 }
    _archie_direct_enroll
  " 2>&1)"
  result=$?

  [ "$result" -eq 0 ] || fail "enrollment rejected an address that arrived one retry later:\n$output"
}

test_cleanup_deletes_the_nl80211_interface_with_iw() {
  local output
  local trace="$SANDBOX/cleanup-trace"
  {
    eval "$(sed '/^case \${1:-} in/,$d' "$host_helper")"
    RUNTIME="$SANDBOX/run"
    AP_IF=archie0
    mkdir -p "$RUNTIME"
    systemctl() { printf 'systemctl %s\n' "$*" >> "$trace"; }
    iw() { printf 'iw %s\n' "$*" >> "$trace"; }
    nft() { printf 'nft %s\n' "$*" >> "$trace"; }
    ip() { printf 'ip %s\n' "$*" >> "$trace"; }
    cleanup_runtime
  }
  output="$(cat "$trace")"

  case "$output" in
    *'iw dev archie0 del'*) ;;
    *) fail "cleanup did not delete the virtual Wi-Fi interface with iw:\n$output" ;;
  esac
}

test_baseline_refuses_to_measure_while_direct_wifi_is_active() {
  command -v zsh >/dev/null 2>&1 || return 0

  local output result
  output="$(zsh -f -c "
    source '$controller'
    ipconfig() { print -r -- 10.77.78.1 }
    _archie_direct_benchmark baseline
  " 2>&1)"
  result=$?

  [ "$result" -ne 0 ] || fail "baseline accepted the direct AP as a baseline network"
  case "$output" in
    *'stop the direct AP'*) ;;
    *) fail "baseline failure did not explain how to recover:\n$output" ;;
  esac
}
