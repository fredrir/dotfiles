#!/usr/bin/env bash

FORMAT_CHANGED=0

conf_mode() {
  case "$1" in
    */hypr/*|*/hypr-local.conf|hypr*.conf) printf '%s\n' hypr ;;
    */kitty/colors-*.conf|*/colors-*.conf) printf '%s\n' plain ;;
    */kitty/*.conf|*/kitty.conf) printf '%s\n' kitty ;;
    *) printf '%s\n' plain ;;
  esac
}

format_conf_stream() {
  awk -v mode="$(conf_mode "$1")" -f "$LIB/format.awk"
}

format_conf_file() {
  local file="$1" temp
  [ -f "$file" ] || die "not a file: $file"
  case "$file" in
    *.conf) ;;
    *) die "not a .conf file: $file" ;;
  esac
  temp="$(mktemp "${TMPDIR:-/tmp}/dotfile-format.XXXXXX")"
  format_conf_stream "$file" < "$file" > "$temp"
  if ! cmp -s "$file" "$temp"; then
    command cat "$temp" > "$file"
    log "  format ${file#"$DOTFILES/"}"
    FORMAT_CHANGED=$((FORMAT_CHANGED + 1))
  fi
  rm "$temp"
}

tracked_conf_files() {
  git -C "$DOTFILES" ls-files -z -- '*.conf'
}

cmd_format() {
  if [ "${1:-}" = "--stdin" ]; then
    [ "$#" -eq 2 ] || die "usage: dotfile format --stdin <filename>"
    format_conf_stream "$2"
    return 0
  fi

  local paths=() path file
  if [ "$#" -eq 0 ]; then
    while IFS= read -rd '' path; do
      paths+=("$DOTFILES/$path")
    done < <(tracked_conf_files)
  else
    for path in "$@"; do
      if [ -d "$path" ]; then
        while IFS= read -rd '' file; do
          paths+=("$file")
        done < <(find "$path" -type f -name '*.conf' -print0)
      else
        paths+=("$path")
      fi
    done
  fi

  [ "${#paths[@]}" -gt 0 ] || die "no .conf files found"
  for file in "${paths[@]}"; do
    format_conf_file "$file"
  done
  log "formatted $FORMAT_CHANGED of ${#paths[@]} config files"
}
