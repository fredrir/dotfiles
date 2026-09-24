#!/usr/bin/env bash

resolve_dotfile() {
  local root="$1" candidate selected=""
  for candidate in "$root/scripts/rust/target/debug/dotfile" "$root/scripts/rust/target/commands/dotfile" "$root/scripts/rust/target/release/dotfile" "$DOTFILES_COMPILED/dotfile"; do
    [ -x "$candidate" ] || continue
    if [ -z "$selected" ] || [ "$candidate" -nt "$selected" ]; then
      selected="$candidate"
    fi
  done
  [ -n "$selected" ] || return 1
  printf '%s\n' "$selected"
}
