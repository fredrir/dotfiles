#!/usr/bin/env bash
set -euo pipefail

DOTFILES="${DOTFILE_ROOT:-$HOME/dotfiles}"
BIN="$DOTFILES/.bin"
BUILT="$DOTFILES/scripts/rust/target/commands/dotfile"
STAGED=""

cleanup() { [ -z "$STAGED" ] || rm -f -- "$STAGED"; }
trap cleanup EXIT

if ! command -v cargo >/dev/null 2>&1; then
  echo "setup: cargo is required to build dotfile (https://rustup.rs)" >&2
  exit 1
fi

cargo build --profile commands --locked --quiet \
  --manifest-path "$DOTFILES/scripts/rust/Cargo.toml" --bin dotfile

mkdir -p "$BIN"
if ! cmp -s "$BUILT" "$BIN/dotfile"; then
  STAGED="$(mktemp "$BIN/.dotfile.XXXXXX")"
  install -m 0755 "$BUILT" "$STAGED"
  mv -f "$STAGED" "$BIN/dotfile"
  STAGED=""
fi

exec "$BIN/dotfile" sync "$@"
