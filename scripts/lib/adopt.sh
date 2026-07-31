#!/usr/bin/env bash

declare -A ADOPT_GROUPS=(
  [--shared]=shared
  [--linux]=linux/common
  [--kde]=linux/kde
  [--hyprland]=linux/hyprland
  [--server]=linux/server
  [--macos]=macos
)

locate_source() {
  local path="$1" matches
  if [ -e "$path" ] || [ -L "$path" ]; then
    case "$path" in
      /*) printf '%s\n' "$path" ;;
      *) printf '%s\n' "$PWD/$path" ;;
    esac
    return 0
  fi
  if [ -e "$HOME/.config/$path" ]; then
    printf '%s\n' "$HOME/.config/$path"
    return 0
  fi
  matches=("$HOME/.config/$path"*)
  if [ "${#matches[@]}" -eq 1 ]; then
    printf '%s\n' "${matches[0]}"
    return 0
  fi
  if [ "${#matches[@]}" -gt 1 ]; then
    die "ambiguous, matches: ${matches[*]#"$HOME/.config/"}"
  fi
  die "not found: $path (looked in ~/.config)"
}

refuse_managed_source() {
  local src="$1" link
  if [ -L "$src" ]; then
    if owned_by_repo "$(resolve_link "$src")"; then
      die "already managed: $(tilde "$src")"
    fi
    die "refusing to adopt a foreign symlink: $(tilde "$src")"
  fi
  if [ -d "$src" ]; then
    while IFS= read -rd '' link; do
      if owned_by_repo "$(resolve_link "$link")"; then
        die "already partially managed ($(tilde "$link")), add individual files instead"
      fi
    done < <(find "$src" -type l -print0)
  fi
}

ADOPT_PKG=""
ADOPT_DESTREL=""
ADOPT_MAPLINE=""

plan_destination() {
  local src="$1" group="$2" pkgflag="$3" rel pkg destrel mapline=""
  case "$src" in
    "$HOME/.config/"*)
      rel="${src#"$HOME/.config/"}"
      case "$rel" in
        */*)
          pkg="${pkgflag:-${rel%%/*}}"
          destrel="$group/$pkg/${rel#*/}"
          ;;
        *)
          if [ -d "$src" ]; then
            pkg="${pkgflag:-$rel}"
            destrel="$group/$pkg"
            if [ -e "$DOTFILES/$destrel" ]; then
              die "package exists: $destrel (add files inside it individually)"
            fi
          else
            pkg="${pkgflag:-${rel%%.*}}"
            destrel="$group/$pkg/$rel"
            mapline="$group/$pkg = ~/.config"
          fi
          ;;
      esac
      ;;
    "$HOME/"*)
      [ -n "$pkgflag" ] || die "files outside ~/.config need --pkg <name>"
      pkg="$pkgflag"
      destrel="$group/$pkg/$(basename "$src")"
      mapline="$destrel = $(tilde "$src")"
      ;;
    *) die "source must live under \$HOME" ;;
  esac
  ADOPT_PKG="$pkg"
  ADOPT_DESTREL="$destrel"
  ADOPT_MAPLINE="$mapline"
}

warn_if_group_unlinked() {
  local group="$1" profile manifest
  profile="$(resolve_profile "")"
  manifest="$DOTFILES/environment/$profile/manifest"
  [ -f "$manifest" ] || return 0
  if ! manifest_groups "$manifest" | grep -xF "$group" >/dev/null; then
    log "note: group '$group' is not in environment/$profile/manifest, it will not be linked by 'dotfile link' on this machine"
  fi
}

cmd_add() {
  local group="shared" pkgflag="" description="" path=""
  while [ $# -gt 0 ]; do
    case "$1" in
      --pkg)
        shift
        pkgflag="${1:-}"
        [ -n "$pkgflag" ] || die "--pkg needs a name"
        ;;
      --description|--desc)
        shift
        description="${1:-}"
        [ -n "$description" ] || die "--description needs text"
        ;;
      -*)
        [ -n "${ADOPT_GROUPS[$1]:-}" ] || die "unknown flag: $1"
        group="${ADOPT_GROUPS[$1]}"
        ;;
      *)
        [ -z "$path" ] || die "expected a single path"
        path="$1"
        ;;
    esac
    shift
  done
  [ -n "$path" ] || die "usage: dotfile add [flags] <path>"
  case "$description" in
    *$'\n'*|*$'\r'*) die "description must be a single line" ;;
  esac

  local src
  src="$(locate_source "${path/#\~/$HOME}")"
  src="$(canon "$src")"
  refuse_managed_source "$src"

  load_targets
  plan_destination "$src" "$group" "$pkgflag"
  local pkg="$ADOPT_PKG" destrel="$ADOPT_DESTREL" mapline="$ADOPT_MAPLINE"

  local dest="$DOTFILES/$destrel"
  [ -e "$dest" ] && die "destination exists: $destrel"

  load_package_metadata
  if [ -n "$description" ]; then
    set_package_description "$group/$pkg" "$description"
  fi

  mkdir -p "$(dirname "$dest")"
  mv "$src" "$dest"
  ln -s "$dest" "$src"
  log "moved  $(tilde "$src") -> $destrel"
  log "linked $(tilde "$src")"

  if [ -n "$mapline" ] && ! grep -qxF "$mapline" "$TARGETS_FILE" 2>/dev/null; then
    printf '%s\n' "$mapline" >> "$TARGETS_FILE"
    log "mapped $mapline"
  fi

  git -C "$DOTFILES" add "$dest" "$TARGETS_FILE" 2>/dev/null || true
  PACKAGE_FILES_CHANGED=0
  sync_packages
  git -C "$DOTFILES" add "$PACKAGES_CONFIG" "$PACKAGES_DOC" 2>/dev/null || true

  warn_if_group_unlinked "$group"
}
