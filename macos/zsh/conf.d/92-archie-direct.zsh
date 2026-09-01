# Explicit controller for Archie's disabled-by-default shared AP. SSH
# route selection never calls this: changing Macie's Wi-Fi remains a visible
# user action through `archie-direct start`.

_archie_direct_usage() {
  cat <<'EOF'
usage: archie-direct enroll
       archie-direct start shared
       archie-direct status [--json]
       archie-direct stop
       archie-direct benchmark baseline|shared
EOF
}

_archie_direct_profile_saved() {
  networksetup -listpreferredwirelessnetworks en0 2>/dev/null |
    tail -n +2 | sed 's/^[[:space:]]*//' | grep -qxF archie-direct
}

_archie_direct_set_network() {
  local limit=${1:-20} pid tick
  [[ $(ipconfig getifaddr en0 2>/dev/null) == 10.77.78.1 ]] && return 0

  # Tahoe can leave networksetup alive after the association has completed.
  # Watch the actual DHCP address instead of treating process exit as success,
  # and disown the helper so an interactive zsh prints no job notifications.
  networksetup -setairportnetwork en0 archie-direct >/dev/null 2>&1 &|
  pid=$!
  for ((tick = 1; tick <= limit; tick++)); do
    sleep 1
    if [[ $(ipconfig getifaddr en0 2>/dev/null) == 10.77.78.1 ]]; then
      kill "$pid" 2>/dev/null || true
      wait "$pid" 2>/dev/null || true
      return 0
    fi
  done
  kill "$pid" 2>/dev/null || true
  sleep 1
  kill -KILL "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  return 1
}

_archie_direct_remote_start() {
  ssh -tt archie 'sudo systemctl --no-block start archie-direct@shared.service'
}

_archie_direct_join() {
  local attempt
  # Two bounded attempts leave room for one transient association failure
  # without returning to the old multi-minute nested retry behavior.
  for attempt in 1 2; do
    _archie_direct_set_network 20 && return 0
  done
  print -u2 -- 'archie-direct: Macie did not acquire 10.77.78.1'
  return 1
}

_archie_direct_wait_address() {
  local limit=${1:-15} attempt
  for ((attempt = 1; attempt <= limit; attempt++)); do
    [[ $(ipconfig getifaddr en0 2>/dev/null) == 10.77.78.1 ]] && return 0
    sleep 1
  done
  return 1
}

_archie_direct_wait_home() {
  local attempt
  for attempt in {1..45}; do
    if ~/.ssh/bin/home-lan-connect --probe archie.local 22 >/dev/null 2>&1 ||
      /usr/bin/nc -4 -z -G 1 100.126.231.24 22 >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  print -u2 -- 'archie-direct: Archie did not return on LAN or Tailscale'
  return 1
}

_archie_direct_status() {
  local format=${1:-text} target
  for target in wifi-archie archie; do
    if ssh -o BatchMode=yes -o ConnectTimeout=2 -T "$target" \
      "/usr/local/libexec/archie-direct-host status $format" 2>/dev/null; then
      return 0
    fi
  done
  [[ "$format" == json ]] && print -r -- '{"active":false,"archie_reachable":false}' ||
    print -r -- 'inactive (Archie is not reachable)'
  return 1
}

_archie_direct_enroll() {
  local old_clipboard secret_file="$HOME/dotfiles/vars.enc.yaml"
  [[ -r "$secret_file" ]] || {
    print -u2 -- "archie-direct: $secret_file is missing"
    return 1
  }
  _archie_direct_remote_start || return

  old_clipboard=$(pbpaste 2>/dev/null || true)
  if ! sops -d --extract '["archie_direct_sae_password"]' "$secret_file" 2>/dev/null |
    sed -e 's/^"//' -e 's/"$//' | tr -d '\n' | pbcopy; then
    print -u2 -- 'archie-direct: could not place the SAE password on the clipboard'
    return 1
  fi
  open 'x-apple.systempreferences:com.apple.wifi-settings-extension' >/dev/null 2>&1 || true
  print -r -- 'Join “archie-direct” in Wi-Fi Settings and paste the password.'
  print -r -- 'Press Return after Macie shows it is connected.'
  read -r
  print -rn -- "$old_clipboard" | pbcopy
  unset old_clipboard
  _archie_direct_wait_address 15 || {
    print -u2 -- 'archie-direct: enrollment did not obtain 10.77.78.1'
    return 1
  }
  print -r -- 'archie-direct: preferred network saved; shared mode remains active'
  print -r -- 'Run `archie-direct stop` before the baseline benchmark.'
}

_archie_direct_start() {
  local link_status
  _archie_direct_profile_saved || {
    print -u2 -- 'archie-direct: run `archie-direct enroll` first'
    return 1
  }
  _archie_direct_remote_start || true
  _archie_direct_join || return
  ssh -o BatchMode=yes -o ConnectTimeout=4 -T wifi-archie true || {
    print -u2 -- 'archie-direct: DHCP succeeded but direct SSH did not'
    return 1
  }
  link_status=$(ssh -o BatchMode=yes -o ConnectTimeout=4 -T wifi-archie \
    '/usr/local/libexec/archie-direct-host status json' 2>/dev/null) || return
  print -r -- "$link_status" | jq -e \
    '.active == true and .mode == "shared"' >/dev/null || {
    print -u2 -- "archie-direct: requested shared mode, got ${link_status:-no status}"
    return 1
  }
  print -r -- "$link_status" | jq -r \
    '"\(.mode)  \(.address)  channel \(.channel)  \(.width_mhz) MHz  \(.clients) client(s)"'
}

_archie_direct_stop() {
  local target
  for target in wifi-archie archie; do
    if ssh -tt -o ConnectTimeout=3 "$target" \
      'sudo systemctl --no-block stop archie-direct@shared.service'; then
      break
    fi
  done
  _archie_direct_wait_home
}

_archie_direct_summary() {
  local directory=$1
  local -a json_files=("$directory"/hwire-*.json(N))
  (($#json_files)) || return 0
  jq -s '
    def median: sort | .[length / 2 | floor];
    map(.runs[0]) | group_by(.route) | map({
      route: .[0].route,
      latency_ms: ([.[] | select(.streams == 1) | .latency_ms.p50] | median),
      p99_ms: ([.[] | select(.streams == 1) | .latency_ms.p99] | median),
      single_up_bps: ([.[] | select(.streams == 1) | .up.bits_per_second] | median),
      single_down_bps: ([.[] | select(.streams == 1) | .down.bits_per_second] | median),
      four_up_bps: ([.[] | select(.streams == 4) | .up.bits_per_second] | median),
      four_down_bps: ([.[] | select(.streams == 4) | .down.bits_per_second] | median)
    })' "${json_files[@]}" >"$directory/summary.json"
  {
    print '# Archie direct-link benchmark'
    print
    print '| route | p50 RTT | p99 RTT | 1-stream up/down | 4-stream up/down |'
    print '|---|---:|---:|---:|---:|'
    jq -r '.[] | "| \(.route) | \(.latency_ms | tostring) ms | \(.p99_ms | tostring) ms | \((.single_up_bps / 1000000 | floor)) / \((.single_down_bps / 1000000 | floor)) Mbit/s | \((.four_up_bps / 1000000 | floor)) / \((.four_down_bps / 1000000 | floor)) Mbit/s |"' \
      "$directory/summary.json"
  } >"$directory/summary.md"
}

_archie_direct_benchmark() {
  local label=$1 timestamp directory route run streams target peer link_state
  local -i measured=0
  local -a routes
  if [[ "$label" == baseline && $(ipconfig getifaddr en0 2>/dev/null) == 10.77.78.1 ]]; then
    print -u2 -- 'archie-direct: stop the direct AP before the baseline benchmark'
    return 1
  fi
  if [[ "$label" == shared ]]; then
    link_state=$(_archie_direct_status json) || return
    print -r -- "$link_state" | jq -e --arg mode "$label" \
      '.active == true and .mode == $mode' >/dev/null || {
      print -u2 -- "archie-direct: start $label mode before benchmarking it"
      return 1
    }
  fi
  timestamp=$(date -u +%Y%m%dT%H%M%SZ)
  directory="${XDG_STATE_HOME:-$HOME/.local/state}/archie-direct/benchmarks/${timestamp}-${label}"
  mkdir -p "$directory"
  case "$label" in
  baseline)
    routes=(cable lan tailscale)
    target=archie
    ;;
  shared)
    routes=(wifi)
    target=wifi-archie
    ;;
  *)
    _archie_direct_usage
    return 2
    ;;
  esac

  system_profiler SPAirPortDataType >"$directory/macie-wifi.txt" 2>&1
  ssh -o BatchMode=yes -o ConnectTimeout=4 -T "$target" \
    'iw dev; iw dev archie0 station dump 2>/dev/null || true; hostapd_cli -i archie0 status 2>/dev/null || true' \
    >"$directory/archie-wifi.txt" 2>&1 || true

  for route in "${routes[@]}"; do
    hwire --route "$route" --latency --samples 20 >/dev/null 2>&1 || continue
    for run in {1..5}; do
      for streams in 1 4; do
        hwire --route "$route" --time 10 --samples 1000 --streams "$streams" --json \
          >"$directory/hwire-${route}-p${streams}-${run}.json" || return
      done
    done
    ((measured++))
    case "$route" in
    cable) peer=10.77.77.2 ;;
    wifi) peer=10.77.78.2 ;;
    lan) peer=$(~/.ssh/bin/home-lan-connect --resolve archie.local | awk '{print $2}') ;;
    tailscale) peer=100.126.231.24 ;;
    esac
    ping -c 200 -i 0.1 "$peer" >"$directory/ping-${route}.txt" 2>&1 || true
  done

  if ((! measured)); then
    print -u2 -- "archie-direct: no $label routes were reachable; no benchmark was recorded"
    print -u2 -- "$directory"
    return 1
  fi

  if [[ "$label" == shared ]]; then
    scutil --dns >"$directory/dns.txt" 2>&1
    dscacheutil -q host -a name example.com >"$directory/dns-resolution.txt" 2>&1 || true
    if curl -fsSIL --max-time 10 https://example.com >"$directory/internet.txt" 2>&1; then
      print -r -- available >"$directory/internet-result.txt"
    else
      print -r -- unavailable >"$directory/internet-result.txt"
    fi
  fi
  _archie_direct_summary "$directory"
  print -r -- "$directory"
}

archie-direct() {
  emulate -L zsh
  local command=${1:-} argument=${2:-}
  case "$command" in
  enroll)
    (($# == 1)) || {
      _archie_direct_usage
      return 2
    }
    _archie_direct_enroll
    ;;
  start)
    (($# == 2)) && [[ "$argument" == shared ]] || {
      _archie_direct_usage
      return 2
    }
    _archie_direct_start
    ;;
  status)
    (($# <= 2)) || {
      _archie_direct_usage
      return 2
    }
    [[ -z "$argument" || "$argument" == --json ]] || {
      _archie_direct_usage
      return 2
    }
    _archie_direct_status "${argument:+json}"
    ;;
  stop)
    (($# == 1)) || {
      _archie_direct_usage
      return 2
    }
    _archie_direct_stop
    ;;
  benchmark)
    (($# == 2)) && [[ "$argument" == baseline || "$argument" == shared ]] || {
      _archie_direct_usage
      return 2
    }
    _archie_direct_benchmark "$argument"
    ;;
  -h | --help | help) _archie_direct_usage ;;
  *)
    _archie_direct_usage
    return 2
    ;;
  esac
}
