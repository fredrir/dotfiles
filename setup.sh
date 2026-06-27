#!/usr/bin/env bash
set -euo pipefail

# Per-profile dotfiles installer.
#
#   ./setup.sh [profile]
#
# `profile` is a path under hosts/ following machine/distro/desktop, e.g.
#   desktop/arch-linux/kde-hyprland | laptop/arch-linux/hyprland | macbook/macos
# It defaults to `hostname -s`. The profile must have hosts/<profile>/manifest
# listing the stow groups to apply (one per line).
#
# Each group (shared, linux/common, linux/kde, linux/hyprland, macos) holds GNU
# Stow packages; every package is stowed into $HOME. A package directory with a
# `.nostow` marker is skipped (reference-only or generated assets).

DOTFILES="$(cd "$(dirname "$0")" && pwd)"
PROFILE="${1:-$(hostname -s)}"
MANIFEST="$DOTFILES/hosts/$PROFILE/manifest"

if ! command -v stow >/dev/null 2>&1; then
  echo "error: GNU stow is not installed." >&2
  exit 1
fi

if [ ! -f "$MANIFEST" ]; then
  echo "error: no manifest for profile '$PROFILE' (expected $MANIFEST)." >&2
  echo "Available profiles:" >&2
  ( cd "$DOTFILES/hosts" && find . -name manifest | sed 's|^\./||; s|/manifest$||' | sort ) \
    | while IFS= read -r p; do echo "  ./setup.sh $p"; done >&2
  exit 1
fi

echo "Setting up profile '$PROFILE' from $DOTFILES"

failed=""   # accumulates "group/pkg" entries that hit stow conflicts

# Stow every package in each group named by the manifest.
while IFS= read -r line || [ -n "$line" ]; do
  group="${line%%#*}"                           # strip comments
  group="${group#"${group%%[![:space:]]*}"}"    # ltrim
  group="${group%"${group##*[![:space:]]}"}"     # rtrim
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
    # zsh is split across groups (shared/zsh + linux/common/zsh + linux/hyprland/zsh
    # + macos/zsh all co-populate ~/.config/zsh/conf.d). Stow it with --no-folding
    # so the target subtree is real directories + per-file symlinks; otherwise the
    # first group folds ~/.config/zsh into one symlink and the next group sees a
    # foreign target and aborts. Other packages fold normally (single dir symlink).
    fold=""
    [ "$name" = "zsh" ] && fold="--no-folding"
    # Capture in an `if` so a conflict doesn't trip `set -e` and abort the whole
    # run — warn, record it, and keep stowing the remaining packages.
    if out="$( cd "$dir" && stow --target="$HOME" $fold --restow "$name" 2>&1 )"; then
      echo "  stowed: $group/$name"
    else
      echo "  !! CONFLICT — skipped $group/$name:"
      printf '%s\n' "$out" | sed 's/^/       /'
      failed="$failed $group/$name"
    fi
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

  # Profile-specific Hyprland override -> conf.d/local.conf (sourced last).
  HOSTCONF="$DOTFILES/hosts/$PROFILE/hypr-local.conf"
  if [ -f "$HOSTCONF" ]; then
    mkdir -p "$HOME/.config/hypr/conf.d"
    ln -sf "$HOSTCONF" "$HOME/.config/hypr/conf.d/local.conf"
    echo "  linked hosts/$PROFILE/hypr-local.conf -> ~/.config/hypr/conf.d/local.conf"
  else
    echo "  note: no hosts/$PROFILE/hypr-local.conf (using defaults; set monitors/env there)"
  fi

  if [ ! -f "$HOME/.config/hypr/wallpaper.png" ]; then
    echo "  note: place your wallpaper at ~/.config/hypr/wallpaper.png"
  fi

  command -v hyprctl >/dev/null 2>&1 && hyprctl reload >/dev/null 2>&1 || true
fi

if [ -n "$failed" ]; then
  echo
  echo "Finished with conflicts ($PROFILE). Skipped:"
  for f in $failed; do echo "  - $f"; done
  echo "Those targets already exist and aren't owned by stow — usually a stale"
  echo "broken symlink from a previous layout, or a real file. Resolve and re-run:"
  echo "  rm <broken-symlink>                       # if it's a dead link"
  echo "  mv ~/.config/<name> ~/config-conflicts/   # if it's a real file/dir"
  echo "  ./setup.sh $PROFILE"
  exit 1
fi

echo "Done ($PROFILE)."
