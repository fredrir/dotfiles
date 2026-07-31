REMOVE_GROUP=""
REMOVE_PACKAGE=""
REMOVE_PACKAGE_ROOT=""
REMOVE_REL=""
REMOVE_SOURCE=""

locate_remove_source() {
  local input="$1" rel group best="" rest pkg source
  case "$input" in
    "$DOTFILES") die "path must include a package" ;;
    "$DOTFILES"/*) rel="${input#"$DOTFILES"/}" ;;
    /*) rel="${input#/}" ;;
    ./*) rel="${input#./}" ;;
    *) rel="$input" ;;
  esac
  rel="$(canon "/$rel")"
  rel="${rel#/}"
  [ -n "$rel" ] || die "path must include a package"

  while IFS= read -r group; do
    case "$rel" in
      "$group"/*)
        if [ "${#group}" -gt "${#best}" ]; then best="$group"; fi
        ;;
    esac
  done < <(package_groups)
  [ -n "$best" ] || die "not a package path: $input"

  rest="${rel#"$best"/}"
  pkg="${rest%%/*}"
  [ -n "$pkg" ] || die "path must include a package"
  [ "$pkg" != "overrides" ] || die "override paths cannot be removed as packages"

  source="$DOTFILES/$rel"
  [ -e "$source" ] || [ -L "$source" ] || die "not found in dotfiles: $rel"
  [ -d "$DOTFILES/$best/$pkg" ] || die "not a package path: $rel"

  REMOVE_GROUP="$best"
  REMOVE_PACKAGE="$pkg"
  REMOVE_PACKAGE_ROOT="$DOTFILES/$best/$pkg"
  REMOVE_REL="$rel"
  REMOVE_SOURCE="$source"
}

remove_destination() {
  local full="$1" pkg="$2" rel="$3" destination
  destination="$(map_dst "$full" "$pkg" "$rel")"
  owned_by_repo "$destination" && die "target for $full points inside dotfiles"
  printf '%s\n' "$destination"
}

existing_remove_parent() {
  local destination="$1" parent
  parent="$(dirname "$destination")"
  while [ ! -e "$parent" ] && [ ! -L "$parent" ]; do
    [ "$parent" != "/" ] || break
    parent="$(dirname "$parent")"
  done
  printf '%s\n' "$parent"
}

validate_remove_node() {
  local source="$1" full="$2" pkg="$3" rel="$4" entry name
  remove_destination "$full" "$pkg" "$rel" >/dev/null
  if [ ! -d "$source" ] || [ -L "$source" ]; then return 0; fi
  for entry in "$source"/*; do
    name="$(basename "$entry")"
    validate_remove_node "$entry" "$full/$name" "$pkg" "${rel:+$rel/}$name"
  done
}

discard_remove_source() {
  local source="$1" destination="$2"
  if [ -d "$source" ] && [ ! -L "$source" ]; then
    rm -r "$source"
  else
    rm "$source"
  fi
  log "kept   existing $(tilde "$destination")"
}

unfold_remove_ancestors() {
  local source="$1" destination="$2" parent current="/" segment resolved
  local segments=()
  parent="$(dirname "$destination")"
  IFS=/ read -ra segments <<< "${parent#/}"
  for segment in "${segments[@]}"; do
    [ -n "$segment" ] || continue
    current="${current%/}/$segment"
    [ -L "$current" ] || continue
    resolved="$(resolve_link "$current")"
    case "$source" in
      "$resolved"/*)
        [ -d "$resolved" ] || die "managed parent is not a directory: $(tilde "$current")"
        unfold "$current" "$resolved"
        ;;
    esac
  done
}

materialize_remove_node() {
  local source="$1" full="$2" pkg="$3" rel="$4" destination current entry name parent
  destination="$(remove_destination "$full" "$pkg" "$rel")"
  unfold_remove_ancestors "$source" "$destination"

  if [ -d "$source" ] && [ ! -L "$source" ]; then
    if [ -L "$destination" ]; then
      current="$(resolve_link "$destination")"
      if [ "$current" != "$source" ]; then
        discard_remove_source "$source" "$destination"
        return 0
      elif has_target_under "$full" || never_fold "$destination"; then
        unfold "$destination" "$source"
      else
        rm "$destination"
        mv "$source" "$destination"
        log "kept   $(tilde "$destination")"
        return 0
      fi
    elif [ -e "$destination" ] && [ ! -d "$destination" ]; then
      discard_remove_source "$source" "$destination"
      return 0
    fi
    if [ ! -e "$destination" ]; then
      parent="$(existing_remove_parent "$destination")"
      if [ ! -d "$parent" ]; then
        discard_remove_source "$source" "$parent"
        return 0
      fi
      if ! has_target_under "$full" && ! never_fold "$destination"; then
        mkdir -p "$(dirname "$destination")"
        mv "$source" "$destination"
        log "kept   $(tilde "$destination")"
        return 0
      fi
      mkdir -p "$destination"
    fi
    for entry in "$source"/*; do
      name="$(basename "$entry")"
      materialize_remove_node "$entry" "$full/$name" "$pkg" "${rel:+$rel/}$name"
    done
    rmdir "$source"
    return 0
  fi

  if [ -L "$destination" ]; then
    current="$(resolve_link "$destination")"
    if [ "$current" = "$source" ]; then
      rm "$destination"
    else
      discard_remove_source "$source" "$destination"
      return 0
    fi
  elif [ -e "$destination" ]; then
    discard_remove_source "$source" "$destination"
    return 0
  else
    parent="$(existing_remove_parent "$destination")"
    if [ ! -d "$parent" ]; then
      discard_remove_source "$source" "$parent"
      return 0
    fi
  fi
  mkdir -p "$(dirname "$destination")"
  mv "$source" "$destination"
  log "kept   $(tilde "$destination")"
}

remove_target_entries() {
  local prefix="$1" temp raw key changed=0
  [ -f "$TARGETS_FILE" ] || return 0
  temp="$(mktemp "${TMPDIR:-/tmp}/dotfile-targets.XXXXXX")"
  while IFS= read -r raw || [ -n "$raw" ]; do
    case "$raw" in
      *=*)
        key="$(trim "${raw%%=*}")"
        if [ "$key" = "$prefix" ] || [ "${key#"$prefix"/}" != "$key" ]; then
          changed=1
          log "unmapped $key"
          continue
        fi
        ;;
    esac
    printf '%s\n' "$raw" >> "$temp"
  done < "$TARGETS_FILE"
  if [ "$changed" = 1 ]; then
    mv "$temp" "$TARGETS_FILE"
  else
    rm "$temp"
  fi
}

prune_empty_package_dirs() {
  local directory="$1"
  while [ "$directory" = "$REMOVE_PACKAGE_ROOT" ] || [ "${directory#"$REMOVE_PACKAGE_ROOT"/}" != "$directory" ]; do
    if [ -d "$directory" ]; then
      rmdir "$directory" 2>/dev/null || return 0
    fi
    [ "$directory" != "$REMOVE_PACKAGE_ROOT" ] || return 0
    directory="$(dirname "$directory")"
  done
}

cmd_remove() {
  [ "$#" -eq 1 ] || die "usage: dotfile remove <path>"
  locate_remove_source "$1"
  load_targets
  load_package_metadata
  validate_package_names

  local node_rel
  if [ "$REMOVE_SOURCE" = "$REMOVE_PACKAGE_ROOT" ]; then
    node_rel=""
  else
    node_rel="${REMOVE_SOURCE#"$REMOVE_PACKAGE_ROOT"/}"
  fi

  validate_remove_node "$REMOVE_SOURCE" "$REMOVE_REL" "$REMOVE_PACKAGE" "$node_rel"
  local source_parent
  source_parent="$(dirname "$REMOVE_SOURCE")"
  materialize_remove_node "$REMOVE_SOURCE" "$REMOVE_REL" "$REMOVE_PACKAGE" "$node_rel"
  prune_empty_package_dirs "$source_parent"
  remove_target_entries "$REMOVE_REL"

  PACKAGE_FILES_CHANGED=0
  sync_packages
  git -C "$DOTFILES" add -A -- "$REMOVE_REL" "$TARGETS_FILE" "$PACKAGES_CONFIG" "$PACKAGES_DOC" 2>/dev/null || true
  log "removed $REMOVE_REL from dotfiles"
}
