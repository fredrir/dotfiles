#!/usr/bin/env bash
set -euo pipefail

DOTFILES="$(cd "$(dirname "$0")" && pwd)"
PROFILE="${1:-$(uname -n)}"

git -C "$DOTFILES" config core.hooksPath "$DOTFILES/.githooks" 2>/dev/null || true

mkdir -p "$HOME/.local/bin"
ln -sf "$DOTFILES/scripts/dotfile" "$HOME/.local/bin/dotfile"

link_failed=0
"$DOTFILES/scripts/dotfile" link "$PROFILE" || link_failed=1

MANIFEST="$DOTFILES/hosts/$PROFILE/manifest"

if command -v systemctl >/dev/null 2>&1 && [ -f "$HOME/.config/systemd/user/generate-theme.path" ]; then
  systemctl --user daemon-reload 2>/dev/null || true
  if systemctl --user enable --now generate-theme.path 2>/dev/null; then
    echo "  enabled theme auto-regenerate watcher"
  fi
fi

if [ -f "$MANIFEST" ] && grep -qE '(^|[[:space:]])linux/hyprland([[:space:]]|$)' "$MANIFEST"; then
  ELEPHANT_SRC="$DOTFILES/linux/hyprland/elephant/files.toml"
  if [ -f "$ELEPHANT_SRC" ]; then
    mkdir -p "$HOME/.config/elephant"
    sed "s|\$HOME|$HOME|g" "$ELEPHANT_SRC" > "$HOME/.config/elephant/files.toml"
    echo "  generated ~/.config/elephant/files.toml"
  fi

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

exit "$link_failed"
