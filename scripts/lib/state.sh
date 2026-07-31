#!/usr/bin/env bash

declare -A OVERRIDES=()
LINK_GROUPS=()
ACTIVE_OVERRIDE_DIRS=()

saved_profile() {
  if [ -f "$STATE_DIR/profile" ]; then cat "$STATE_DIR/profile"; fi
}

resolve_profile() {
  local profile="${1:-}"
  if [ -z "$profile" ]; then profile="$(saved_profile)"; fi
  printf '%s\n' "$profile"
}

save_profile() {
  if [ "$DRY" = 1 ]; then return 0; fi
  mkdir -p "$STATE_DIR"
  printf '%s\n' "$1" > "$STATE_DIR/profile"
}

list_profiles() {
  ( cd "$DOTFILES/environment" && find . -name manifest | sed 's|^\./||; s|/manifest$||' | LC_ALL=C sort )
}

require_manifest() {
  local profile="$1" manifest="$DOTFILES/environment/$1/manifest"
  if [ -n "$profile" ] && [ -f "$manifest" ]; then
    printf '%s\n' "$manifest"
    return 0
  fi
  {
    if [ -z "$profile" ]; then
      echo "dotfile: no profile selected (run ./setup.sh or pass one)"
    else
      echo "dotfile: no manifest for profile '$profile'"
    fi
    echo "available profiles:"
    list_profiles | sed 's/^/  /'
  } >&2
  exit 1
}

manifest_groups() {
  local line
  while IFS= read -r line || [ -n "$line" ]; do
    line="$(trim "${line%%#*}")"
    if [ -n "$line" ]; then printf '%s\n' "$line"; fi
  done < "$1"
  return 0
}

load_overrides() {
  OVERRIDES=()
  [ -f "$OVERRIDES_FILE" ] || return 0
  local line
  while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in *=*) ;; *) continue ;; esac
    OVERRIDES["${line%%=*}"]="${line#*=}"
  done < "$OVERRIDES_FILE"
}

save_overrides() {
  if [ "$DRY" = 1 ]; then return 0; fi
  mkdir -p "$STATE_DIR"
  local group
  for group in "${!OVERRIDES[@]}"; do
    printf '%s=%s\n' "$group" "${OVERRIDES[$group]}"
  done | LC_ALL=C sort > "$OVERRIDES_FILE"
}

available_overrides() {
  local dir out=""
  for dir in "$DOTFILES/$1/overrides"/*/; do
    out="$out${out:+ }$(basename "${dir%/}")"
  done
  printf '%s\n' "$out"
}

select_override() {
  local group="$1" name="$2"
  [ -d "$DOTFILES/$group/overrides" ] || die "group has no overrides: $group"
  if [ "$name" != "none" ] && [ ! -d "$DOTFILES/$group/overrides/$name" ]; then
    die "unknown override '$name' for $group (available: $(available_overrides "$group"))"
  fi
  OVERRIDES["$group"]="$name"
}

collect_groups() {
  local manifest="$1" group name
  LINK_GROUPS=()
  ACTIVE_OVERRIDE_DIRS=()
  while IFS= read -r group; do
    LINK_GROUPS+=("$group")
    [ -d "$DOTFILES/$group/overrides" ] || continue
    name="${OVERRIDES[$group]:-}"
    if [ -z "$name" ]; then
      log "  note: '$group' has machine overrides ($(available_overrides "$group")), none selected"
      log "        select one: dotfile link --override $group=<name>  (or =none)"
      continue
    fi
    if [ "$name" = "none" ]; then continue; fi
    if [ -d "$DOTFILES/$group/overrides/$name" ]; then
      LINK_GROUPS+=("$group/overrides/$name")
      ACTIVE_OVERRIDE_DIRS+=("$DOTFILES/$group/overrides/$name")
    else
      log "  skip missing override: $group/overrides/$name"
    fi
  done < <(manifest_groups "$manifest")
}

each_package() {
  local group dir pkgdir pkg
  for group in ${LINK_GROUPS[@]+"${LINK_GROUPS[@]}"}; do
    dir="$DOTFILES/$group"
    if [ ! -d "$dir" ]; then
      printf 'no-group\t\t%s\n' "$group"
      continue
    fi
    for pkgdir in "$dir"/*/; do
      pkgdir="${pkgdir%/}"
      pkg="$(basename "$pkgdir")"
      if [ "$pkg" = "overrides" ]; then continue; fi
      if [ -e "$pkgdir/.nolink" ]; then
        printf 'nolink\t%s\t%s\n' "$pkgdir" "$group/$pkg"
      else
        printf 'link\t%s\t%s\n' "$pkgdir" "$group/$pkg"
      fi
    done
  done
}
