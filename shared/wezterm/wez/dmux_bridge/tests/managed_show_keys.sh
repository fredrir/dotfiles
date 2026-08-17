#!/bin/sh
set -eu

wezterm_bin=${DMUX_WEZTERM_BIN:?set DMUX_WEZTERM_BIN to the maintained fork wezterm-gui binary}
repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/../../../../.." && pwd)
runtime=$(mktemp -d)
trap 'rm -rf "$runtime"' EXIT HUP INT TERM

mkdir -p "$runtime/bridge"
umask 077
printf '0123456789abcdef0123456789abcdef' >"$runtime/bridge/key"
case "$(uname -s)" in
Darwin)
  fixture_boot_id='macos:1700000000:123456'
  fixture_start_token='macos:1700000100:654321'
  ;;
*)
  fixture_boot_id='linux:77777777-7777-4777-8777-777777777777'
  fixture_start_token='linux:123456'
  ;;
esac
printf '%s\n' \
  "{\"descriptor_version\":1,\"state\":\"ready\",\"epoch\":\"55555555-5555-4555-8555-555555555555\",\"pid\":4242,\"socket\":\"$runtime/dmux/wez-dmux.sock\",\"socket_dev\":42,\"socket_ino\":84,\"start_token\":\"$fixture_start_token\",\"backend_instance_uid\":\"44444444-4444-4444-8444-444444444444\",\"boot_nonce\":\"66666666-6666-4666-8666-666666666666\",\"boot_id\":\"$fixture_boot_id\",\"written_by\":\"mux-startup\",\"written_at\":\"2026-08-17T12:34:56Z\",\"sentinel_window_id\":1,\"sentinel_tab_id\":2,\"sentinel_pane_id\":3,\"sentinel_fallback\":false}" \
  >"$runtime/wez-dmux.json"

keys=$(
  DMUX_RUNTIME_DIR="$runtime" \
  DMUX_WEZ_FIRST=1 \
  DMUX_BIN=/usr/bin/false \
  DMUX_TEST_CONFIG_ROOT="$repo_root" \
  DMUX_TEST_DESCRIPTOR_FIXTURE="$runtime/wez-dmux.json" \
  "$wezterm_bin" \
    --config-file "$repo_root/shared/wezterm/wez/dmux_bridge/tests/show_keys_config.lua" \
    show-keys --lua 2>"$runtime/show-keys.stderr"
)

if printf '%s\n' "$keys" \
  | rg 'Spawn(Window|Tab|Pane|Command)|AttachDomain|DetachDomain|Switch(ToWorkspace|WorkspaceRelative)|QuitApplication|HideApplication|CloseCurrent(Tab|Pane)|ActivateWindowRelative|MoveTab|RotatePanes|AdjustPaneSize|TogglePaneZoomState'
then
  cat "$runtime/show-keys.stderr" >&2
  echo 'unsafe managed GUI key action found' >&2
  exit 1
fi

printf '%s\n' "$keys" | rg -q "key = 'q', mods = 'SUPER', action = act.EmitEvent"
printf '%s\n' "$keys" | rg -q "key = 'w', mods = 'SUPER', action = act.EmitEvent"
printf '%s\n' "$keys" | rg -q "key = 'W', mods = 'CTRL', action = act.EmitEvent"
printf '%s\n' "$keys" | rg -q "key = 'F4', mods = 'ALT', action = act.EmitEvent"
if printf '%s\n' "$keys" | rg -q "key = 'Q', mods = 'SUPER'"; then
  echo 'Command+Shift+Q must remain unbound in managed mode' >&2
  exit 1
fi

echo 'dmux maintained-fork show-keys test: managed lifecycle and creation surfaces are broker-only'
