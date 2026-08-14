#!/usr/bin/env bash

SOURCE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"

setup_sandbox() {
  SANDBOX="$(mktemp -d "${TMPDIR:-/tmp}/dotfile-test.XXXXXX")"
  REPO="$SANDBOX/repo"
  HOME="$SANDBOX/home"
  XDG_CONFIG_HOME="$HOME/.config"
  ZSH="$HOME/.oh-my-zsh"
  export HOME XDG_CONFIG_HOME ZSH
  export DOTFILE_ROOT="$REPO"
  mkdir -p "$REPO/config" "$HOME/.config" "$HOME/.local/share" "$HOME/.local/bin"
  : > "$REPO/config/targets.dotfile"
}

teardown_sandbox() {
  case "${SANDBOX:-}" in
    /tmp/*|"${TMPDIR:-/tmp}"/*) rm -rf "$SANDBOX" ;;
  esac
}

fail() {
  printf '    %s\n' "$*" >&2
  exit 1
}

require_sandboxed_home() {
  case "$HOME" in
    "$SANDBOX"/*) ;;
    *) printf 'REFUSING TO RUN: HOME=%s is outside the sandbox\n' "$HOME" >&2; exit 99 ;;
  esac
}

dotfile() {
  require_sandboxed_home
  OUTPUT="$("$SOURCE_ROOT/scripts/python/.venv/bin/dotfile" "$@" 2>&1)"
  STATUS=$?
  return 0
}

mkpkg() {
  local path="$REPO/$1" content="${2-}"
  mkdir -p "$(dirname "$path")"
  printf '%s\n' "$content" > "$path"
}

mkprofile() {
  local name="$1"
  shift
  mkdir -p "$REPO/environment/$name"
  printf '%s\n' "$@" > "$REPO/environment/$name/manifest"
}

target() {
  printf '%s\n' "$1" >> "$REPO/config/targets.dotfile"
}

assert_symlink() {
  local path="$1" want="$2" got
  [ -L "$path" ] || fail "expected symlink at $path (found: $(describe "$path"))"
  got="$(readlink "$path")"
  [ "$got" = "$want" ] || fail "symlink $path -> $got, wanted $want"
}

assert_realdir() {
  local path="$1"
  [ -L "$path" ] && fail "expected real directory at $path, found symlink -> $(readlink "$path")"
  [ -d "$path" ] || fail "expected directory at $path (found: $(describe "$path"))"
}

assert_absent() {
  local path="$1"
  if [ -e "$path" ] || [ -L "$path" ]; then
    fail "expected nothing at $path (found: $(describe "$path"))"
  fi
}

assert_exists() {
  local path="$1"
  if [ ! -e "$path" ] && [ ! -L "$path" ]; then
    fail "expected something at $path"
  fi
}

assert_file_is() {
  local path="$1" want="$2" got
  got="$(cat "$path")"
  [ "$got" = "$want" ] || fail "$path contents:
--- got ---
$got
--- want ---
$want"
}

assert_ok() {
  [ "$STATUS" -eq 0 ] || fail "expected success, got exit $STATUS:
$OUTPUT"
}

assert_fails() {
  [ "$STATUS" -ne 0 ] || fail "expected failure, got exit 0:
$OUTPUT"
}

assert_output_has() {
  case "$OUTPUT" in
    *"$1"*) ;;
    *) fail "expected output to contain '$1':
$OUTPUT" ;;
  esac
}

assert_output_lacks() {
  case "$OUTPUT" in
    *"$1"*) fail "expected output not to contain '$1':
$OUTPUT" ;;
  esac
}

describe() {
  local path="$1"
  if [ -L "$path" ]; then printf 'symlink -> %s' "$(readlink "$path")"
  elif [ -d "$path" ]; then printf 'directory'
  elif [ -f "$path" ]; then printf 'regular file'
  else printf 'nothing'
  fi
}
