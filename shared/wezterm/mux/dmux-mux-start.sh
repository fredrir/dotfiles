#!/bin/sh
# dmux managed mux-server service wrapper (plan §15.1, ADR 001/002/004).
#
# Invoked by launchd (macOS: macos/launchd/com.fredrir.wezterm-mux.plist) or
# systemd (Arch: linux/arch/wezterm-mux/wezterm-mux.service). The service
# manager is the ONLY legitimate starter: WezTerm has no socket mutual
# exclusion, so a manually started second server on the same path silently
# steals the socket and orphans the original (ADR 002). Never run this by
# hand while the service is loaded.
#
# Responsibilities:
#   - resolve the per-user runtime dir exactly the way dmux does (§10.1);
#   - mint a fresh server epoch + boot nonce per start; mux-startup's native
#     publisher derives the OS process/boot/socket witness after exec;
#   - when the feature flag is on, resolve the durable Wez backend instance
#     and provide the resurrection manifest/spool inputs to mux-startup;
#   - exec wezterm-mux-server FOREGROUND (--daemonize contends the shared
#     default ~/.local/share/wezterm pid lock, ADR 004; the service manager
#     supplies restart/serialization).
set -eu
umask 077

here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
repo=$(CDPATH='' cd -- "$here/../../.." && pwd)

fail() {
  echo "dmux-mux-start: $*" >&2
  exit 1
}

# Runtime dir, mirroring dmux_runtime_dir() (§10.1): macOS uses
# confstr(_CS_DARWIN_USER_TEMP_DIR) + "dmux" (getconf is the sh equivalent),
# Linux requires XDG_RUNTIME_DIR. Both give short paths, keeping the socket
# well under the ~104-byte sun_path limit (ADR 001).
case "$(uname -s)" in
Darwin)
  base=$(getconf DARWIN_USER_TEMP_DIR) || fail 'getconf DARWIN_USER_TEMP_DIR failed'
  runtime="${base%/}/dmux"
  ;;
*)
  [ -n "${XDG_RUNTIME_DIR:-}" ] || fail 'XDG_RUNTIME_DIR is required on Linux'
  runtime="${XDG_RUNTIME_DIR%/}/dmux"
  ;;
esac
case "$runtime" in
/*) ;;
*) fail 'resolved dmux runtime directory is not absolute' ;;
esac
# The maintained mux server's early `--dmux-managed-service` bootstrap owns
# secure creation/validation of the fixed runtime directory and prebinding of
# its socket through held dirfds. Shell pathname mutation here would re-open a
# symlink/swap window before native authority exists.

sock="$runtime/wez-dmux.sock"

# Durable per-host policy (ADR 012 WS-F.1; three-valued flag, ADR 010 §5:
# 1 states Wez-first, 0 states legacy, unset/empty states no preference).
# Precedence for DMUX_WEZ_FIRST and DMUX_LEGACY_POLICY, highest first:
#   1. a NON-EMPTY value already in this process's environment. A service
#      manager that deliberately passes DMUX_WEZ_FIRST=0 is stating legacy
#      and must not be overridden by a file; an empty value states nothing.
#   2. on macOS, the untracked ~/.config/dmux/service.env -- the same file
#      the com.fredrir.dmux-env LaunchAgent copies into the launchd session
#      for the GUI. Reading it here too means the mux never depends on
#      LaunchAgent ordering. Because rule 1 wins, after editing the file run
#      `launchctl kickstart gui/$UID/com.fredrir.dmux-env` BEFORE restarting
#      this job, or launchd's stale copy is what arrives here; `dmux doctor`
#      shows every layer. A malformed file applies nothing (warned below;
#      the loader refused the same file), so the host stays consistently on
#      the tracked default rather than half-on.
#   3. the tracked default below: 0 until the §21 step 9 flip, when it and
#      WEZ_FIRST_BY_DEFAULT in main.rs move to Wez-first together.
# On Linux ~/.config/environment.d/50-dmux.conf is the ONE knob: the systemd
# user manager passes it to this unit and to the graphical session it
# starts, so service.env is deliberately not read there -- two sources would
# be two knobs. The shared parser never evals or sources the file.
. "$here/dmux-service-env.sh"
if [ "$(uname -s)" = Darwin ] && service_env=$(dmux_service_env_path); then
  if service_env_lines=$(dmux_service_env_lines "$service_env"); then
    if [ -z "${DMUX_WEZ_FIRST:-}" ] &&
      value=$(dmux_service_env_lookup DMUX_WEZ_FIRST "$service_env_lines"); then
      DMUX_WEZ_FIRST=$value
    fi
    if [ -z "${DMUX_LEGACY_POLICY:-}" ] &&
      value=$(dmux_service_env_lookup DMUX_LEGACY_POLICY "$service_env_lines"); then
      DMUX_LEGACY_POLICY=$value
      export DMUX_LEGACY_POLICY
    fi
  else
    echo "dmux-mux-start: WARN ignoring malformed $service_env; tracked defaults apply" >&2
  fi
fi
DMUX_WEZ_FIRST="${DMUX_WEZ_FIRST:-0}"
export DMUX_WEZ_FIRST

lowercase() { printf '%s' "$1" | tr '[:upper:]' '[:lower:]'; }
epoch=$(lowercase "$(uuidgen)")
boot_nonce=$(lowercase "$(uuidgen)")

# Resolve a dmux binary that actually provides the hidden `_mux-idle`
# sentinel command (an older installed dmux may predate it, and a sentinel
# that exits would leave the managed mux empty). launchd/systemd PATH is
# minimal, so probe explicit candidates too.
dmux_bin=''
for candidate in \
  "$(command -v dmux 2>/dev/null || true)" \
  "$HOME/.local/bin/dmux" \
  "$repo/scripts/rust/target/debug/dmux" \
  "$repo/scripts/rust/target/release/dmux"; do
  [ -n "$candidate" ] && [ -x "$candidate" ] || continue
  if "$candidate" _mux-idle --help >/dev/null 2>&1; then
    dmux_bin="$candidate"
    break
  fi
done
if [ -z "$dmux_bin" ]; then
  # Not fatal: the Lua handler falls back to a shell idle-loop sentinel and
  # publishes failed/unavailable rather than a structurally weak ready
  # descriptor.
  echo 'dmux-mux-start: WARN no dmux with _mux-idle found; Lua sentinel fallback will be used' >&2
fi

# P9's acknowledged GUI bridge key must exist before a feature-gated GUI
# evaluates its config.  The command creates/reuses the raw 0600 file and is
# intentionally silent; never capture or print the key in this shell.
if [ "$DMUX_WEZ_FIRST" = 1 ]; then
  [ -n "$dmux_bin" ] || fail 'DMUX_WEZ_FIRST=1 requires dmux'
  "$dmux_bin" _bridge-key >/dev/null || fail 'could not ensure mandatory GUI bridge key'
fi

# Descriptor publication is a §15.1 invariant in both rollout states, but a
# disabled rollout must not create registry identity just to become ready.
# The native publisher accepts an absent UID only for starting/failed.
backend_instance="${DMUX_BACKEND_INSTANCE:-}"
pane_bootstrap=''
if [ "$DMUX_WEZ_FIRST" = 1 ]; then
  [ -n "$dmux_bin" ] || fail 'DMUX_WEZ_FIRST=1 requires dmux'
  if [ -z "$backend_instance" ]; then
    case "$(uname -s)" in
    Darwin) service_label='com.fredrir.wezterm-mux' ;;
    *) service_label='wezterm-mux.service' ;;
    esac
    backend_instance=$(
      "$dmux_bin" _recovery instance \
        --socket "$sock" \
        --service-label "$service_label"
    ) || fail 'could not resolve durable Wez backend instance'
  fi
  [ -n "$backend_instance" ] || fail 'DMUX_WEZ_FIRST=1 requires a durable backend instance'

  pane_bootstrap=$(dirname -- "$dmux_bin")/pane-bootstrap
  [ -x "$pane_bootstrap" ] || pane_bootstrap=$(command -v pane-bootstrap 2>/dev/null || true)
  [ -n "$pane_bootstrap" ] && [ -x "$pane_bootstrap" ] || \
    fail 'DMUX_WEZ_FIRST=1 requires pane-bootstrap beside dmux or on PATH'
fi

if [ -n "$backend_instance" ]; then
  case "$backend_instance" in
  ????????-????-????-????-????????????) ;;
  *) fail 'managed Wez backend instance is not a UUID' ;;
  esac
fi

wez_mux=''
for candidate in \
  "${DMUX_WEZTERM_MUX_SERVER:-}" \
  "$(command -v wezterm-mux-server 2>/dev/null || true)" \
  /opt/homebrew/bin/wezterm-mux-server \
  /usr/local/bin/wezterm-mux-server \
  /usr/bin/wezterm-mux-server; do
  [ -n "$candidate" ] && [ -x "$candidate" ] || continue
  wez_mux="$candidate"
  break
done
[ -n "$wez_mux" ] || fail 'wezterm-mux-server not found'

DMUX_SOCKET="$sock"
DMUX_RUNTIME_DIR="$runtime"
DMUX_SERVER_EPOCH="$epoch"
DMUX_BOOT_NONCE="$boot_nonce"
DMUX_BACKEND_INSTANCE="$backend_instance"
DMUX_BIN="$dmux_bin"
DMUX_PANE_BOOTSTRAP="$pane_bootstrap"
export DMUX_SOCKET DMUX_RUNTIME_DIR DMUX_SERVER_EPOCH \
  DMUX_BOOT_NONCE DMUX_BACKEND_INSTANCE DMUX_BIN \
  DMUX_PANE_BOOTSTRAP DMUX_WEZ_FIRST

# Hygiene: never inherit pane/endpoint identity from whoever started us.
# WEZTERM_UNIX_SOCKET does NOT set a server's listen socket (ADR 004) -- only
# unix_domains[].socket_path in the config file does -- but a leaked value
# could misdirect any child CLI invocation.
unset WEZTERM_UNIX_SOCKET WEZTERM_PANE TMUX TMUX_PANE 2>/dev/null || true

exec "$wez_mux" --dmux-managed-service --config-file "$here/dmux-mux.lua"
