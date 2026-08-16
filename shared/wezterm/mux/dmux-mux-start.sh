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
#   - mint a fresh server epoch + boot nonce + process start token per start;
#   - pre-write a `starting` descriptor stub (socket dev/ino unknown here:
#     dmux verifies socket identity itself at probe time per ADR 001);
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
  runtime="$XDG_RUNTIME_DIR/dmux"
  ;;
esac
mkdir -p "$runtime"
chmod 0700 "$runtime"

sock="$runtime/wez-dmux.sock"
descriptor="$runtime/wez-dmux.json"

lowercase() { printf '%s' "$1" | tr '[:upper:]' '[:lower:]'; }
epoch=$(lowercase "$(uuidgen)")
boot_nonce=$(lowercase "$(uuidgen)")
# Start token: after exec below, $$ IS the server pid, so the token binds the
# descriptor to this exact server incarnation.
start_token="$$-$(date +%s)-$(lowercase "$(uuidgen)")"

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
  # records sentinel_fallback=true in the descriptor.
  echo 'dmux-mux-start: WARN no dmux with _mux-idle found; Lua sentinel fallback will be used' >&2
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

# Pre-write the `starting` stub atomically. mux-startup overwrites it with
# the authoritative `starting` -> `ready` records; if the server dies before
# the handler runs, this stub (state=starting, matching start_token) is what
# a reader finds, which is the honest state.
stub_tmp="$descriptor.tmp.$$"
printf '{"descriptor_version":1,"state":"starting","epoch":"%s","pid":%d,"socket":"%s","socket_dev":null,"socket_ino":null,"start_token":"%s","backend_instance_uid":"%s","boot_nonce":"%s","written_by":"wrapper","written_at":"%s"}\n' \
  "$epoch" "$$" "$sock" "$start_token" "${DMUX_BACKEND_INSTANCE:-}" "$boot_nonce" \
  "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >"$stub_tmp"
mv -f "$stub_tmp" "$descriptor"

DMUX_SOCKET="$sock"
DMUX_RUNTIME_DIR="$runtime"
DMUX_DESCRIPTOR="$descriptor"
DMUX_SERVER_EPOCH="$epoch"
DMUX_START_TOKEN="$start_token"
DMUX_BOOT_NONCE="$boot_nonce"
DMUX_BACKEND_INSTANCE="${DMUX_BACKEND_INSTANCE:-}"
DMUX_BIN="$dmux_bin"
export DMUX_SOCKET DMUX_RUNTIME_DIR DMUX_DESCRIPTOR DMUX_SERVER_EPOCH \
  DMUX_START_TOKEN DMUX_BOOT_NONCE DMUX_BACKEND_INSTANCE DMUX_BIN

# Hygiene: never inherit pane/endpoint identity from whoever started us.
# WEZTERM_UNIX_SOCKET does NOT set a server's listen socket (ADR 004) -- only
# unix_domains[].socket_path in the config file does -- but a leaked value
# could misdirect any child CLI invocation.
unset WEZTERM_UNIX_SOCKET WEZTERM_PANE TMUX TMUX_PANE 2>/dev/null || true

exec "$wez_mux" --config-file "$here/dmux-mux.lua"
