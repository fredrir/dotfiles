#!/usr/bin/env bash

declare -A TARGETS=()

load_targets() {
  TARGETS=()
  [ -f "$TARGETS_FILE" ] || return 0
  local line key value
  while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in *=*) ;; *) continue ;; esac
    key="$(trim "${line%%=*}")"
    value="$(trim "${line#*=}")"
    TARGETS["$key"]="${value/#\~/$HOME}"
  done < "$TARGETS_FILE"
}

map_dst() {
  local full="$1" pkg="$2" rel="$3" key best=""
  for key in "${!TARGETS[@]}"; do
    if [ "$full" = "$key" ] || [ "${full#"$key"/}" != "$full" ]; then
      if [ "${#key}" -gt "${#best}" ]; then best="$key"; fi
    fi
  done
  if [ -z "$best" ]; then
    printf '%s\n' "$HOME/.config/$pkg${rel:+/$rel}"
  elif [ "$full" = "$best" ]; then
    printf '%s\n' "${TARGETS[$best]}"
  else
    printf '%s\n' "${TARGETS[$best]}/${full#"$best"/}"
  fi
}

has_target_under() {
  local prefix="$1" key
  for key in "${!TARGETS[@]}"; do
    if [ "${key#"$prefix"/}" != "$key" ]; then return 0; fi
  done
  return 1
}

never_fold() {
  local path="$1" protected
  for protected in \
    "$HOME" \
    "$HOME/.config" \
    "$HOME/.local" \
    "$HOME/.local/share" \
    "$HOME/.local/bin" \
    "$HOME/.config/systemd" \
    "$HOME/.config/systemd/user"
  do
    if [ "$path" = "$protected" ]; then return 0; fi
  done
  return 1
}
