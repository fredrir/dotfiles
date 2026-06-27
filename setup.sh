#!/usr/bin/env bash
set -euo pipefail

# Per-host dotfiles installer.
#
#   ./setup.sh [host]
#
# `host` defaults to `hostname -s`. It must have a manifest at
# hosts/<host>/manifest listing the stow groups to apply (one per line).
# Each group (e.g. shared, linux/common, linux/kde, linux/hyprland, macos) holds
# GNU Stow packages; every package is stowed into $HOME. A package directory
# containing a `.nostow` marker is skipped (reference-only or generated assets).

DOTFILES="$(cd "$(dirname "$0")" && pwd)"
HOST="${1:-$(hostname -s)}"
MANIFEST="$DOTFILES/hosts/$HOST/manifest"

if ! command -v stow >/dev/null 2>&1; then
  echo "error: GNU stow is not installed." >&2
  exit 1
fi

if [ ! -f "$MANIFEST" ]; then
  echo "error: no manifest for host '$HOST' (expected $MANIFEST)." >&2
  echo "Create it with one stow group per line, e.g.:" >&2
  echo "  shared" >&2
  echo "  linux/common" >&2
  echo "  linux/hyprland" >&2
  echo "Known hosts: $(ls "$DOTFILES/hosts" 2>/dev/null | tr '\n' ' ')" >&2
  exit 1
fi

echo "Setting up host '$HOST' from $DOTFILES"

# Stow every package in each group named by the manifest.
while IFS= read -r line || [ -n "$line" ]; do
  group="${line%%#*}"                 # strip comments
  group="${group#"${group%%[![:space:]]*}"}"   # ltrim
  group="${group%"${group##*[![:space:]]}"}"    # rtrim
  [ -z "$group" ] && continue

  dir="$DOTFILES/$group"
  if [ ! -d "$dir" ]; then
    echo "  skip missing group: $group"
    continue
  fi

  for pkg in "$dir"/*/; do
    [ -d "$pkg" ] || continue
    name="$(basename "$pkg")"
    if [ -e "${pkg}.nostow" ]; then
      echo "  skip (.nostow): $group/$name"
      continue
    fi
    # zsh is split across groups (shared/zsh + linux/common/zsh + macos/zsh all
    # co-populate ~/.config/zsh/conf.d). Stow it with --no-folding so the target
    # subtree is real directories + per-file symlinks; otherwise the first group
    # folds ~/.config/zsh into one symlink and the next group sees a foreign
    # target and aborts. Other packages fold normally (single dir symlink).
    fold=""
    [ "$name" = "zsh" ] && fold="--no-folding"
    ( cd "$dir" && stow --target="$HOME" $fold --restow "$name" )
    echo "  stowed: $group/$name"
  done
done < "$MANIFEST"

# --- Hyprland-only post-steps ---------------------------------------------
if grep -qE '(^|[[:space:]])linux/hyprland([[:space:]]|$)' "$MANIFEST"; then
  # elephant: generate config with $HOME expanded (not stowed).
  ELEPHANT_SRC="$DOTFILES/linux/hyprland/elephant/.config/elephant/files.toml"
  if [ -f "$ELEPHANT_SRC" ]; then
    mkdir -p "$HOME/.config/elephant"
    sed "s|\$HOME|$HOME|g" "$ELEPHANT_SRC" > "$HOME/.config/elephant/files.toml"
    echo "  generated ~/.config/elephant/files.toml"
  fi

  # Host-specific Hyprland override -> conf.d/local.conf (sourced last).
  HOSTCONF="$DOTFILES/hosts/$HOST/hypr-local.conf"
  if [ -f "$HOSTCONF" ]; then
    mkdir -p "$HOME/.config/hypr/conf.d"
    ln -sf "$HOSTCONF" "$HOME/.config/hypr/conf.d/local.conf"
    echo "  linked hosts/$HOST/hypr-local.conf -> ~/.config/hypr/conf.d/local.conf"
  else
    echo "  note: no hosts/$HOST/hypr-local.conf (using defaults; set monitors/env there)"
  fi

  if [ ! -f "$HOME/.config/hypr/wallpaper.png" ]; then
    echo "  note: place your wallpaper at ~/.config/hypr/wallpaper.png"
  fi

  command -v hyprctl >/dev/null 2>&1 && hyprctl reload >/dev/null 2>&1 || true
fi

echo "Done ($HOST)."
