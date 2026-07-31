#!/usr/bin/env bash

declare -A PACKAGE_DESC=()
PACKAGE_FILES_CHANGED=0

DEFAULT_GROUPS=(shared linux/common linux/kde linux/hyprland linux/server macos)

load_package_metadata() {
  PACKAGE_DESC=()
  [ -f "$PACKAGES_CONFIG" ] || return 0
  local raw line group="" name description key number=0
  while IFS= read -r raw || [ -n "$raw" ]; do
    number=$((number + 1))
    line="$(trim "$raw")"
    [ -n "$line" ] || continue
    if [ "$line" = "}" ]; then
      [ -n "$group" ] || die "packages.dotfile:$number: unexpected }"
      group=""
      continue
    fi
    case "$line" in
      *" {")
        [ -z "$group" ] || die "packages.dotfile:$number: nested group"
        group="$(trim "${line% \{}")"
        [ -n "$group" ] || die "packages.dotfile:$number: empty group"
        case "$group" in
          *[!A-Za-z0-9._/-]*) die "packages.dotfile:$number: invalid group: $group" ;;
        esac
        continue
        ;;
    esac
    [ -n "$group" ] || die "packages.dotfile:$number: package outside a group"
    case "$line" in
      *" = "*)
        name="$(trim "${line%% = *}")"
        description="$(trim "${line#* = }")"
        ;;
      *)
        name="$line"
        description=""
        ;;
    esac
    [ -n "$name" ] || die "packages.dotfile:$number: empty package"
    case "$name" in
      *[!A-Za-z0-9._+@-]*) die "packages.dotfile:$number: invalid package: $name" ;;
    esac
    key="$group/$name"
    if [ -n "${PACKAGE_DESC[$key]+set}" ]; then
      die "packages.dotfile:$number: duplicate package: $key"
    fi
    PACKAGE_DESC["$key"]="$description"
  done < "$PACKAGES_CONFIG"
  [ -z "$group" ] || die "packages.dotfile:$number: missing } for $group"
}

set_package_description() {
  PACKAGE_DESC["$1"]="$2"
}

package_groups() {
  {
    printf '%s\n' "${DEFAULT_GROUPS[@]}"
    if [ -d "$DOTFILES/environment" ]; then
      local manifest
      while IFS= read -r manifest; do
        manifest_groups "$manifest"
      done < <(find "$DOTFILES/environment" -type f -name manifest -print | LC_ALL=C sort)
    fi
  } | awk 'NF && !seen[$0]++'
}

group_packages() {
  local pkgdir
  for pkgdir in "$1"/*/; do
    basename "${pkgdir%/}"
  done | LC_ALL=C sort
}

validate_package_names() {
  local group pkg
  while IFS= read -r group; do
    [ -d "$DOTFILES/$group" ] || continue
    while IFS= read -r pkg; do
      case "$pkg" in
        overrides) ;;
        *[!A-Za-z0-9._+@-]*) die "package directory has an unsupported name: $group/$pkg" ;;
      esac
    done < <(group_packages "$DOTFILES/$group")
  done < <(package_groups)
}

replace_package_file() {
  local source="$1" destination="$2" label="$3"
  if [ -f "$destination" ] && cmp -s "$source" "$destination"; then
    rm "$source"
    return 0
  fi
  mv "$source" "$destination"
  PACKAGE_FILES_CHANGED=$((PACKAGE_FILES_CHANGED + 1))
  log "updated $label"
}

render_packages() {
  local config="$1" doc="$2"
  local group pkg description wrote_group=0
  local packages=()
  while IFS= read -r group; do
    [ -d "$DOTFILES/$group" ] || continue
    packages=()
    while IFS= read -r pkg; do
      if [ "$pkg" = "overrides" ]; then continue; fi
      packages+=("$pkg")
    done < <(group_packages "$DOTFILES/$group")
    [ "${#packages[@]}" -gt 0 ] || continue

    if [ "$wrote_group" = 1 ]; then printf '\n' >> "$config"; fi
    printf '%s {\n' "$group" >> "$config"
    printf '\n## `%s`\n\n' "$group" >> "$doc"
    for pkg in "${packages[@]}"; do
      description="${PACKAGE_DESC[$group/$pkg]:-}"
      if [ -n "$description" ]; then
        printf '  %s = %s\n' "$pkg" "$description" >> "$config"
        printf -- '- `%s` — %s\n' "$pkg" "$description" >> "$doc"
      else
        printf '  %s\n' "$pkg" >> "$config"
        printf -- '- `%s`\n' "$pkg" >> "$doc"
      fi
    done
    printf '}\n' >> "$config"
    wrote_group=1
  done < <(package_groups)
}

sync_packages() {
  validate_package_names
  local config doc
  config="$(mktemp "${TMPDIR:-/tmp}/packages.dotfile.XXXXXX")"
  doc="$(mktemp "${TMPDIR:-/tmp}/PACKAGES.md.XXXXXX")"
  render_packages "$config" "$doc"
  replace_package_file "$config" "$PACKAGES_CONFIG" packages.dotfile
  replace_package_file "$doc" "$PACKAGES_DOC" PACKAGES.md
}

cmd_packages() {
  [ "$#" -eq 0 ] || die "usage: dotfile packages"
  PACKAGE_FILES_CHANGED=0
  load_package_metadata
  sync_packages
  if [ "$PACKAGE_FILES_CHANGED" = 0 ]; then
    log "packages are current"
  fi
}
