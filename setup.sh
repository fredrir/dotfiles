#!/usr/bin/env bash
set -euo pipefail
shopt -s nullglob

DOTFILES="$(cd "$(dirname "$0")" && pwd)"
ENVDIR="$DOTFILES/environment"
STATE_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/dotfile"

BOLD=$'\033[1m'
DIM=$'\033[2m'
CYAN=$'\033[36m'
RESET=$'\033[0m'
PICKED=""

interactive() { [ -t 0 ] && [ -t 1 ]; }

list_profiles() {
  ( cd "$ENVDIR" && find . -name manifest | sed 's|^\./||; s|/manifest$||' | LC_ALL=C sort )
}

migrate_profile() {
  case "$1" in
    desktop/arch-linux/kde) printf 'arch-linux/kde\n' ;;
    desktop/arch-linux/kde-hyprland) printf 'arch-linux/kde-hyprland\n' ;;
    laptop/arch-linux/hyprland) printf 'arch-linux/hyprland\n' ;;
    macbook/macos) printf 'macos\n' ;;
    vps/linux) printf 'ubuntu/server\n' ;;
    *) printf '%s\n' "$1" ;;
  esac
}

saved_profile() {
  if [ -f "$STATE_DIR/profile" ]; then
    migrate_profile "$(cat "$STATE_DIR/profile")"
  fi
}

saved_override() {
  [ -f "$STATE_DIR/overrides" ] || return 0
  local line
  while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in
      "$1="*)
        printf '%s\n' "${line#*=}"
        return 0
        ;;
    esac
  done < "$STATE_DIR/overrides"
}

pick() {
  local title="$1" default="$2"
  shift 2
  local opts=("$@") count=$# idx=0 first=1 i key rest
  for i in "${!opts[@]}"; do
    [ "${opts[$i]}" = "$default" ] && idx="$i"
  done
  printf '\n  %s%s%s\n' "$BOLD" "$title" "$RESET"
  printf '  %s↑/↓ move · enter select · q quit%s\n\n' "$DIM" "$RESET"
  printf '\033[?25l'
  while :; do
    if [ "$first" = 0 ]; then
      printf '\033[%dA' "$count"
    fi
    first=0
    for i in "${!opts[@]}"; do
      if [ "$i" -eq "$idx" ]; then
        printf '  %s%s❯ %s%s\033[K\n' "$CYAN" "$BOLD" "${opts[$i]}" "$RESET"
      else
        printf '    %s\033[K\n' "${opts[$i]}"
      fi
    done
    IFS= read -rsn1 key < /dev/tty || key=""
    case "$key" in
      $'\033')
        IFS= read -rsn2 -t 1 rest < /dev/tty || rest=""
        case "$rest" in
          '[A') idx=$(( (idx + count - 1) % count )) ;;
          '[B') idx=$(( (idx + 1) % count )) ;;
        esac
        ;;
      k) idx=$(( (idx + count - 1) % count )) ;;
      j) idx=$(( (idx + 1) % count )) ;;
      [1-9])
        if [ "$key" -le "$count" ]; then
          idx=$((key - 1))
        fi
        ;;
      ''|$'\n'|$'\r') break ;;
      q)
        printf '\033[?25h\n'
        exit 130
        ;;
    esac
  done
  printf '\033[?25h'
  PICKED="${opts[$idx]}"
}

if interactive; then
  trap 'printf "\033[?25h"' EXIT
fi

git -C "$DOTFILES" config core.hooksPath "$DOTFILES/.githooks" 2>/dev/null || true

mkdir -p "$HOME/.local/bin"
ln -sf "$DOTFILES/scripts/dotfile" "$HOME/.local/bin/dotfile"

PROFILE="${1:-}"

if [ -z "$PROFILE" ]; then
  if interactive; then
    profiles=()
    while IFS= read -r p; do
      profiles+=("$p")
    done < <(list_profiles)
    default="$(saved_profile)"
    if [ -z "$default" ]; then
      case "$(uname -s)" in
        Darwin) default="macos" ;;
      esac
    fi
    pick "select environment" "$default" "${profiles[@]}"
    PROFILE="$PICKED"
  else
    PROFILE="$(saved_profile)"
    if [ -z "$PROFILE" ]; then
      echo "usage: ./setup.sh [profile]" >&2
      echo "available profiles:" >&2
      list_profiles | sed 's/^/  /' >&2
      exit 1
    fi
  fi
fi

MANIFEST="$ENVDIR/$PROFILE/manifest"
if [ ! -f "$MANIFEST" ]; then
  echo "setup: no manifest for profile '$PROFILE'" >&2
  echo "available profiles:" >&2
  list_profiles | sed 's/^/  /' >&2
  exit 1
fi

OVERRIDE_ARGS=()
if interactive; then
  while IFS= read -r group; do
    group="${group%%#*}"
    group="${group#"${group%%[![:space:]]*}"}"
    group="${group%"${group##*[![:space:]]}"}"
    [ -n "$group" ] || continue
    [ -d "$DOTFILES/$group/overrides" ] || continue
    names=()
    for d in "$DOTFILES/$group/overrides"/*/; do
      names+=("$(basename "${d%/}")")
    done
    [ "${#names[@]}" -gt 0 ] || continue
    pick "select machine override for $group" "$(saved_override "$group")" "${names[@]}" none
    OVERRIDE_ARGS+=(--override "$group=$PICKED")
  done < "$MANIFEST"
fi

echo
link_failed=0
"$DOTFILES/scripts/dotfile" link "$PROFILE" ${OVERRIDE_ARGS[@]+"${OVERRIDE_ARGS[@]}"} || link_failed=1

if command -v systemctl >/dev/null 2>&1 && [ -f "$HOME/.config/systemd/user/generate-theme.path" ]; then
  systemctl --user daemon-reload 2>/dev/null || true
  if systemctl --user enable --now generate-theme.path 2>/dev/null; then
    echo "  enabled theme auto-regenerate watcher"
  fi
fi

if grep -qE '(^|[[:space:]])linux/hyprland([[:space:]]|$)' "$MANIFEST"; then
  ELEPHANT_SRC="$DOTFILES/linux/hyprland/elephant/files.toml"
  if [ -f "$ELEPHANT_SRC" ]; then
    mkdir -p "$HOME/.config/elephant"
    sed "s|\$HOME|$HOME|g" "$ELEPHANT_SRC" > "$HOME/.config/elephant/files.toml"
    echo "  generated ~/.config/elephant/files.toml"
  fi

  STALE_LOCAL="$DOTFILES/linux/hyprland/hypr/conf.d/local.conf"
  if [ -L "$STALE_LOCAL" ] && [ ! -e "$STALE_LOCAL" ]; then
    rm "$STALE_LOCAL"
  fi

  if [ ! -f "$HOME/.config/hypr/wallpaper.png" ]; then
    echo "  note: place your wallpaper at ~/.config/hypr/wallpaper.png"
  fi

  command -v hyprctl >/dev/null 2>&1 && hyprctl reload >/dev/null 2>&1 || true
fi

exit "$link_failed"
