#!/bin/sh
set -eu

wezterm_bin=${DMUX_WEZTERM_BIN:?set DMUX_WEZTERM_BIN to the maintained fork wezterm-gui binary}
repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/../../../../.." && pwd)
runtime=$(mktemp -d)
trap 'rm -rf "$runtime"' EXIT HUP INT TERM

mkdir -p "$runtime/bridge"
umask 077
printf '0123456789abcdef0123456789abcdef' >"$runtime/bridge/key"
printf '%s\n' \
  "{\"state\":\"ready\",\"socket\":\"$runtime/managed.sock\",\"epoch\":\"55555555-5555-4555-8555-555555555555\",\"backend_instance_uid\":\"44444444-4444-4444-8444-444444444444\"}" \
  >"$runtime/wez-dmux.json"

keys=$(
  DMUX_RUNTIME_DIR="$runtime" \
    DMUX_WEZ_FIRST=1 \
    DMUX_BIN=/usr/bin/false \
    "$wezterm_bin" \
    --config-file "$repo_root/shared/wezterm/wezterm.lua" \
    show-keys --lua 2>/dev/null
)

if printf '%s\n' "$keys" \
  | rg 'Spawn(Window|Tab|Pane|Command)|AttachDomain|DetachDomain|Switch(ToWorkspace|WorkspaceRelative)|QuitApplication|HideApplication|CloseCurrent(Tab|Pane)|ActivateWindowRelative|MoveTab|RotatePanes|AdjustPaneSize|TogglePaneZoomState'
then
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
