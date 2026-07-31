#!/usr/bin/env bash

CONFLICTS=()

conflict() { CONFLICTS+=("$1"); }

link_file() {
  local src="$1" dst="$2" current
  if [ -L "$dst" ]; then
    current="$(resolve_link "$dst")"
    if [ "$current" = "$src" ]; then return 0; fi
    if owned_by_repo "$current"; then
      run rm "$dst"
    else
      conflict "$dst"
      return 0
    fi
  elif [ -e "$dst" ]; then
    conflict "$dst"
    return 0
  fi
  run mkdir -p "$(dirname "$dst")"
  run ln -s "$src" "$dst"
  log "  link $(tilde "$dst")"
}

unfold() {
  local dst="$1" current="$2" entry
  run rm "$dst"
  run mkdir -p "$dst"
  for entry in "$current"/*; do
    run ln -s "$entry" "$dst/$(basename "$entry")"
  done
  log "  split $(tilde "$dst")"
}

link_dir() {
  local src="$1" dst="$2" full="$3" pkg="$4" rel="$5" current entry name
  if [ -L "$dst" ]; then
    current="$(resolve_link "$dst")"
    if [ "$current" = "$src" ]; then
      if has_target_under "$full"; then
        unfold "$dst" "$current"
      else
        return 0
      fi
    elif owned_by_repo "$current"; then
      if [ -d "$current" ]; then
        unfold "$dst" "$current"
      else
        run rm "$dst"
      fi
    else
      conflict "$dst"
      return 0
    fi
  fi
  if [ ! -e "$dst" ]; then
    if has_target_under "$full" || never_fold "$dst"; then
      run mkdir -p "$dst"
    else
      run mkdir -p "$(dirname "$dst")"
      run ln -s "$src" "$dst"
      log "  link $(tilde "$dst")"
      return 0
    fi
  elif [ ! -d "$dst" ]; then
    conflict "$dst"
    return 0
  fi
  for entry in "$src"/*; do
    name="$(basename "$entry")"
    walk_node "$pkg" "${rel:+$rel/}$name" "$entry" "$full/$name"
  done
}

walk_node() {
  local pkg="$1" rel="$2" src="$3" full="$4" dst
  if [ "$(basename "$src")" = ".nolink" ]; then return 0; fi
  dst="$(map_dst "$full" "$pkg" "$rel")"
  if [ -d "$src" ] && [ ! -L "$src" ]; then
    link_dir "$src" "$dst" "$full" "$pkg" "$rel"
  else
    link_file "$src" "$dst"
  fi
}

link_package() {
  local pkgdir="$1" name="$2"
  walk_node "$(basename "$pkgdir")" "" "$pkgdir" "$name"
}

stale_override_link() {
  local current="$1" base active
  case "$current" in
    */overrides/*) ;;
    *) return 1 ;;
  esac
  base="${current%%/overrides/*}"
  [ -d "$base/overrides" ] || return 1
  for active in ${ACTIVE_OVERRIDE_DIRS[@]+"${ACTIVE_OVERRIDE_DIRS[@]}"}; do
    case "$current" in "$active"/*) return 1 ;; esac
  done
  return 0
}

prune() {
  local link current
  while IFS= read -rd '' link; do
    current="$(resolve_link "$link")"
    if ! owned_by_repo "$current"; then continue; fi
    if ! stale_override_link "$current"; then
      if [ -e "$link" ]; then continue; fi
    fi
    run rm "$link"
    log "  prune $(tilde "$link")"
  done < <(
    find "$HOME" -maxdepth 1 -type l -lname "$DOTFILES/*" -print0 2>/dev/null
    find "$HOME/.config" "$HOME/.local" -maxdepth 6 -type l -lname "$DOTFILES/*" -print0 2>/dev/null
  )
}

report_conflicts() {
  local profile="$1"
  if [ "${#CONFLICTS[@]}" -eq 0 ]; then return 0; fi
  echo
  echo "conflicts (existing files not owned by dotfiles):"
  printf '  %s\n' "${CONFLICTS[@]}"
  echo "move each aside and re-run: mv <file> <file>.bak && dotfile link $profile"
  exit 1
}

cmd_link() {
  local profile="" spec group name
  local requested=()
  while [ $# -gt 0 ]; do
    case "$1" in
      -n|--dry-run) DRY=1 ;;
      --override)
        shift
        spec="${1:-}"
        case "$spec" in
          *=*) requested+=("$spec") ;;
          *) die "--override needs <group>=<name|none>" ;;
        esac
        ;;
      -*) die "unknown flag: $1" ;;
      *) profile="$1" ;;
    esac
    shift
  done

  profile="$(resolve_profile "$profile")"
  local manifest
  manifest="$(require_manifest "$profile")"
  load_targets
  load_overrides
  for spec in ${requested[@]+"${requested[@]}"}; do
    group="${spec%%=*}"
    name="${spec#*=}"
    select_override "$group" "$name"
  done

  log "linking profile '$profile'"
  collect_groups "$manifest"
  prune

  local state pkgdir name
  while IFS=$'\t' read -r state pkgdir name; do
    case "$state" in
      link) link_package "$pkgdir" "$name" ;;
      nolink) log "  skip (.nolink): $name" ;;
      no-group) log "  skip missing group: $name" ;;
    esac
  done < <(each_package)

  save_profile "$profile"
  save_overrides
  report_conflicts "$profile"
  log "done"
}

cmd_status() {
  local profile
  profile="$(resolve_profile "${1:-}")"
  local manifest
  manifest="$(require_manifest "$profile")"
  load_targets
  load_overrides
  collect_groups "$manifest"

  local state pkgdir name pkg file rel full dst
  local ok=0 missing=0 differing=0
  while IFS=$'\t' read -r state pkgdir name; do
    if [ "$state" != "link" ]; then continue; fi
    pkg="$(basename "$pkgdir")"
    while IFS= read -rd '' file; do
      if [ "$(basename "$file")" = ".nolink" ]; then continue; fi
      rel="${file#"$pkgdir"/}"
      full="$name/$rel"
      dst="$(map_dst "$full" "$pkg" "$rel")"
      if [ ! -e "$dst" ] && [ ! -L "$dst" ]; then
        log "missing  $(tilde "$dst")"
        missing=$((missing + 1))
      elif [ "$(resolve_path "$dst")" = "$file" ]; then
        ok=$((ok + 1))
      else
        log "differs  $(tilde "$dst")"
        differing=$((differing + 1))
      fi
    done < <(find "$pkgdir" \( -type f -o -type l \) -print0)
  done < <(each_package)

  log "profile '$profile': $ok linked, $missing missing, $differing differing"
  [ $((missing + differing)) -eq 0 ]
}
