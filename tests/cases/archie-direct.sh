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
    _archie_direct_start
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
    _archie_direct_set_network() { (( network_calls++ )); return 0 }
    ipconfig() { return 1 }
    sleep() { (( sleep_calls++ )); return 0 }
    _archie_direct_join >/dev/null 2>&1
    print -r -- \"\$sleep_calls \$network_calls\"
  ")"
  read -r sleeps network_calls <<< "$output"

  [ "$sleeps" -le 45 ] || fail "a failed join consumed $sleeps seconds of retry budget"
  [ "$network_calls" -le 10 ] || fail "a failed join attempted association $network_calls times"
}

test_a_blocked_networksetup_attempt_has_a_wall_clock_timeout() {
  command -v zsh >/dev/null 2>&1 || return 0

  local started elapsed output result
  started="$(date +%s)"
  output="$(zsh -f -c "
    source '$controller'
    networksetup() { command sleep 20 }
    _archie_direct_set_network 1
  " 2>&1)"
  result=$?
  elapsed=$(( $(date +%s) - started ))

  [ "$result" -ne 127 ] || fail "the timed association helper is missing:\n$output"
  [ "$elapsed" -le 4 ] || fail "one networksetup attempt blocked for ${elapsed}s"
}

test_association_allows_tahoe_to_take_nine_seconds() {
  command -v zsh >/dev/null 2>&1 || return 0

  local output result
  output="$(zsh -f -c "
    source '$controller'
    integer ticks=0
    networksetup() { command sleep 20 }
    ipconfig() { (( ticks >= 9 )) && print -r -- 10.77.78.1 }
    sleep() { (( ticks++ )); return 0 }
    _archie_direct_set_network
  " 2>&1)"
  result=$?

  [ "$result" -eq 0 ] ||
    fail "association was killed before Tahoe supplied the direct address:\n$output"
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

test_shared_mode_accepts_ap_forwarding_before_docker_rules() {
  local output
  local trace="$SANDBOX/iptables-trace"
  {
    eval "$(sed '/^case \${1:-} in/,$d' "$host_helper")"
    AP_IF=archie0
    UPLINK_IF=wlp9s0
    iptables() {
      printf 'iptables %s\n' "$*" >> "$trace"
      case "$1" in
        -C) return 1 ;;
      esac
      return 0
    }
    install_docker_forwarding
  }
  output="$(cat "$trace")"

  case "$output" in
    *'iptables -I DOCKER-USER 1 -i archie0 -o wlp9s0 -s 10.77.78.0/30'*) ;;
    *) fail "shared mode did not accept outbound AP forwarding:\n$output" ;;
  esac
  case "$output" in
    *'iptables -I DOCKER-USER 1 -i wlp9s0 -o archie0 -d 10.77.78.0/30'*) ;;
    *) fail "shared mode did not accept established AP replies:\n$output" ;;
  esac
}

test_cleanup_removes_its_docker_forwarding_rules() {
  local output
  local trace="$SANDBOX/iptables-cleanup-trace"
  {
    eval "$(sed '/^case \${1:-} in/,$d' "$host_helper")"
    AP_IF=archie0
    UPLINK_IF=wlp9s0
    iptables() {
      local marker
      printf 'iptables %s\n' "$*" >> "$trace"
      case "$*" in
        *'-i archie0 -o wlp9s0'*) marker="$SANDBOX/outbound-removed" ;;
        *'-i wlp9s0 -o archie0'*) marker="$SANDBOX/reply-removed" ;;
        *) return 1 ;;
      esac
      case "$1" in
        -C) [ ! -e "$marker" ] ;;
        -D) touch "$marker" ;;
        *) return 1 ;;
      esac
    }
    remove_docker_forwarding
  }
  output="$(cat "$trace")"

  case "$output" in
    *'iptables -D DOCKER-USER -i archie0 -o wlp9s0 -s 10.77.78.0/30'*) ;;
    *) fail "cleanup did not remove outbound AP forwarding:\n$output" ;;
  esac
  case "$output" in
    *'iptables -D DOCKER-USER -i wlp9s0 -o archie0 -d 10.77.78.0/30'*) ;;
    *) fail "cleanup did not remove established AP replies:\n$output" ;;
  esac
}

test_only_shared_mode_is_exposed_by_the_controller() {
  command -v zsh >/dev/null 2>&1 || return 0

  local output result
  output="$(zsh -f -c "
    source '$controller'
    _archie_direct_start() { print -r -- started; }
    archie-direct start isolated
  " 2>&1)"
  result=$?

  [ "$result" -eq 2 ] || fail "retired mode returned $result instead of usage failure"
  case "$output" in
    *started*) fail "retired mode reached the start implementation" ;;
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
