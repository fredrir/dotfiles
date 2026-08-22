#!/bin/bash
# Run the dmux test suite isolated from the live runtime directory, and fail
# if the live directory grew (ADR 012 §3.2 / WS-E.1; plan §20.1 "suite runs
# leave the live runtime directory unchanged; a run that grows it fails").
#
#   scripts/rust/crates/dmux/tests/run-isolated.sh            # whole suite, one thread
#   scripts/rust/crates/dmux/tests/run-isolated.sh --test cli # any `cargo test` args
#
# What it does, and why each part exists:
#
# * Exports the owner-side seams to FRESH scratch directories for the whole
#   run: `DMUX_RUNTIME_DIR` (every socket, descriptor, bridge key, bootstrap
#   FIFO and kernel-lock file — `runtime::dmux_runtime_dir()` returns it
#   verbatim), `XDG_DATA_HOME` (the registry) and `XDG_STATE_HOME` (client
#   history). Every process the suite spawns inherits them, so a test that
#   forgets to pass `--lock-dir`/`--data-dir` still cannot reach the live
#   service's directory. `DMUX_WEZ_FIRST` and `DMUX_LEGACY_POLICY` are unset
#   for the run: gated-dispatch tests set them on their own children, and an
#   inherited value would skew what they assert.
#
# * Snapshots the live runtime directory — the seam-blind
#   `runtime::platform_runtime_dir()`: `$(getconf DARWIN_USER_TEMP_DIR)dmux`
#   on macOS, `${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/dmux` on Linux — before
#   and after, RECURSIVELY and including dotfiles. `ls | wc -l` undercounts:
#   `bootstrap/` gains per-pane marker files and the service's lease and
#   replace witnesses are dotfiles. Growth fails the run and names every new
#   entry; an entry that disappeared is reported but does not fail, since the
#   suite never deletes from the live directory and the service may rewrite
#   its own witnesses at any time.
#
# * Keeps `DMUX_RUNTIME_DIR` SHORT, directly under `$TMPDIR`, and refuses to
#   run if it is not. Scratch mux servers bind `<dir>/wez-dmux.sock` beneath
#   the seam, and a unix socket path is limited to `sun_path` — 104 bytes on
#   macOS, 108 on Linux. Pointing the seam at a deep directory (a per-session
#   scratchpad, a worktree path) makes every socket-binding test fail with
#   "File name too long". `TMUX_TMPDIR` is deliberately left alone: the tmux
#   scratch servers use `-L` namespaces under the default socket directory,
#   and pointing it at a long path breaks them the same way.
#
# Why a script and not a test: `cargo test` runs every test binary as its own
# process and has no suite-wide setup or teardown, so nothing inside the
# suite can take one snapshot before the first binary and one after the
# last. The unit-level companion is `tests/runtime_dir_seam.rs`, which proves
# that every constructor honours the seam even under bare `cargo test`.
# A `[env]` table in `.cargo/config.toml` was considered and rejected: it
# would also redirect `cargo run -p dmux` — the operator's own CLI against
# the live service — and a fixed relative path cannot be fresh per run.

set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
workspace=$(cd "$here/../../.." && pwd)

fail() {
  echo "run-isolated: $*" >&2
  exit 2
}

# The live runtime directory, resolved exactly as `platform_runtime_dir()`.
case "$(uname -s)" in
  Darwin)
    base=$(getconf DARWIN_USER_TEMP_DIR) || fail 'getconf DARWIN_USER_TEMP_DIR failed'
    ;;
  Linux)
    base=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}
    ;;
  *)
    fail "unsupported platform $(uname -s)"
    ;;
esac
live="${base%/}/dmux"

# Scratch seams: a fresh, short directory directly under $TMPDIR.
tmp=${TMPDIR:-/tmp}
scratch=$(mktemp -d "${tmp%/}/dmux-t.XXXXXX") || fail 'mktemp failed'
runtime="$scratch/rt"
data="$scratch/xdg"
mkdir -p "$runtime" "$data"

# sun_path is 104 bytes on macOS including the NUL; leave room for the
# socket file name the scratch servers append.
socket="$runtime/wez-dmux.sock"
if [ "${#socket}" -ge 100 ]; then
  fail "DMUX_RUNTIME_DIR $runtime is too deep for a unix socket path (${#socket} bytes; sun_path is 104 on macOS, 108 on Linux) — set TMPDIR to a shorter directory"
fi

snapshot() {
  if [ -d "$live" ]; then
    (cd "$live" && find . -mindepth 1 | LC_ALL=C sort)
  fi
}

before="$scratch/live.before"
after="$scratch/live.after"
snapshot > "$before"
count_before=$(wc -l < "$before" | tr -d ' ')
echo "run-isolated: live runtime dir $live has $count_before entries; seam $runtime"

if [ "$#" -eq 0 ]; then
  set -- -- --test-threads=1
fi

status=0
(
  cd "$workspace"
  unset DMUX_WEZ_FIRST DMUX_LEGACY_POLICY
  DMUX_RUNTIME_DIR="$runtime" \
  XDG_DATA_HOME="$data" \
  XDG_STATE_HOME="$data/state" \
    cargo test -p dmux "$@"
) || status=$?

snapshot > "$after"
count_after=$(wc -l < "$after" | tr -d ' ')
new=$(comm -13 "$before" "$after")
gone=$(comm -23 "$before" "$after")

echo "run-isolated: live runtime dir $live: $count_before entries before, $count_after after"
if [ -n "$gone" ]; then
  echo "run-isolated: entries that disappeared during the run (not a failure):"
  printf '  %s\n' $gone
fi
if [ -n "$new" ]; then
  echo "run-isolated: FAIL — the live runtime dir grew by $(printf '%s\n' "$new" | wc -l | tr -d ' ') entries; a test reached it without the seam:" >&2
  printf '  %s\n' $new >&2
  [ "$status" -eq 0 ] && status=1
fi

if [ "$status" -eq 0 ]; then
  echo "run-isolated: OK — suite green, live runtime dir unchanged"
  rm -rf "$scratch"
else
  echo "run-isolated: exit $status; scratch kept at $scratch" >&2
fi
exit "$status"
