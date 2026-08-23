#!/bin/sh
# com.fredrir.dmux-env LaunchAgent program (ADR 012 WS-F.1).
#
# Copies every KEY=VALUE of the host-local, untracked ~/.config/dmux/service.env
# into the launchd gui session with `launchctl setenv`, so a reboot no longer
# clears the per-host DMUX_WEZ_FIRST canary/cutover flag (plan §21 steps 7
# and 9; ADR 010 §5). launchd runs this once at login (RunAtLoad, no
# KeepAlive); the WezTerm GUI, Hammerspoon and dmux.app read the flag from
# that session environment when they are next launched
# (shared/wezterm/wezterm.lua:9). The managed mux does NOT wait for this job:
# dmux-mux-start.sh reads the same file itself.
#
# Since the §21 step 9 flip (ADR 012 WS-G.7) the tracked default is
# Wez-first: when the file is absent or states no DMUX_WEZ_FIRST, this job
# places DMUX_WEZ_FIRST=1 in the session — the same default
# dmux-mux-start.sh assumes — so the frozen fork's GUI, which only ever tests
# the literal value 1, runs managed on a host with no per-host file. A file
# that states DMUX_WEZ_FIRST=0 wins (ADR 010 §5: 0 is an explicit opt-out;
# unset is "no preference"). A value already in the session is NOT
# consulted: this job is the writer of that session value, and a stale copy
# from before an edit is exactly what a re-run replaces.
#
# Re-run after editing the file, then restart the mux:
#   launchctl kickstart gui/$UID/com.fredrir.dmux-env
#   launchctl kickstart -k gui/$UID/com.fredrir.wezterm-mux   # kills panes
# This job only ever sets; it never unsets. To state legacy, write
# DMUX_WEZ_FIRST=0 rather than deleting the line, or `launchctl setenv` by
# hand (an unsetenv would be re-filled with the default at next login).
#
# The parser lives in dmux-service-env.sh and is shared with the mux wrapper;
# the grammar is documented there. A malformed file is refused WHOLE with a
# nonzero exit: nothing is applied, each bad line is reported by number, and
# `launchctl print gui/$UID/com.fredrir.dmux-env` shows the last exit status.
# `dmux doctor` reports the same file and whether launchd carries its value.
#
# No StandardOutPath/StandardErrorPath in the plist (same hazard as the mux
# job), so stderr goes to launchd's bit bucket; `logger` mirrors each message
# to the unified log (`log show --predicate 'process == "logger"'`, or search
# for dmux-env-load) when it is on PATH.
set -eu

here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
. "$here/dmux-service-env.sh"

log() {
  echo "dmux-env-load: $*" >&2
  if command -v logger >/dev/null 2>&1; then
    logger -t dmux-env-load -- "$*" || true
  fi
}

command -v launchctl >/dev/null 2>&1 || {
  log 'launchctl not found; this loader is for macOS launchd only'
  exit 1
}

file=$(dmux_service_env_path) || {
  log 'neither XDG_CONFIG_HOME nor HOME is set; cannot locate service.env'
  exit 1
}

if ! lines=$(dmux_service_env_lines "$file"); then
  log "refusing $file: malformed; nothing applied"
  exit 1
fi

# The tracked default (plan §21 step 9). The file's own line, if any, comes
# later in the here-document and so is applied after this one, which is what
# makes 0 in the file win.
default_lines='DMUX_WEZ_FIRST=1'
if dmux_service_env_lookup DMUX_WEZ_FIRST "$lines" >/dev/null; then
  default_lines=''
fi

# Every line below was validated against the grammar in dmux-service-env.sh
# (key ^DMUX_[A-Z0-9_]*$, value ^[A-Za-z0-9_./:@+,-]*$). The here-document
# expands $lines as text once; the result is never re-expanded or evaluated.
count=0
while IFS= read -r line; do
  [ -n "$line" ] || continue
  key=${line%%=*}
  value=${line#*=}
  launchctl setenv "$key" "$value" || {
    log "launchctl setenv $key failed; stopping"
    exit 1
  }
  count=$((count + 1))
done <<EOF
$default_lines
$lines
EOF
if [ -n "$default_lines" ]; then
  log "applied $count assignment(s) to the launchd session ($file states no DMUX_WEZ_FIRST; the tracked default 1 applies)"
else
  log "applied $count assignment(s) from $file to the launchd session"
fi
