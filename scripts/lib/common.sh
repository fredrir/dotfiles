#!/usr/bin/env bash

DRY=0

die() { printf 'dotfile: %s\n' "$*" >&2; exit 1; }

log() { printf '%s\n' "$*"; }

run() {
  if [ "$DRY" = 1 ]; then
    log "  would: $*"
  else
    "$@"
  fi
}

tilde() { printf '%s\n' "${1/#$HOME/\~}"; }

trim() {
  local value="$1"
  value="${value#"${value%%[![:space:]]*}"}"
  printf '%s\n' "${value%"${value##*[![:space:]]}"}"
}

canon() {
  local path="$1" segment out="" IFS=/
  for segment in $path; do
    case "$segment" in
      ""|".") ;;
      "..") out="${out%/*}" ;;
      *) out="$out/$segment" ;;
    esac
  done
  printf '%s\n' "${out:-/}"
}

resolve_link() {
  local link="$1" target
  target="$(readlink "$link")"
  case "$target" in
    /*) canon "$target" ;;
    *) canon "$(dirname "$link")/$target" ;;
  esac
}

resolve_path() {
  local dir base path
  dir="$(dirname "$1")"
  base="$(basename "$1")"
  dir="$(cd -P "$dir" 2>/dev/null && pwd -P)" || { printf '%s\n' "$1"; return 0; }
  path="$dir/$base"
  if [ -L "$path" ]; then resolve_link "$path"; else printf '%s\n' "$path"; fi
}

owned_by_repo() {
  case "$1" in
    "$DOTFILES"/*) return 0 ;;
    *) return 1 ;;
  esac
}
