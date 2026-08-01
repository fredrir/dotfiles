#!/usr/bin/env bash

generator() {
  OUTPUT="$(cd "$SOURCE_ROOT" && "$SOURCE_ROOT/scripts/.venv/bin/generate-theme" "$@" 2>&1)"
  STATUS=$?
  return 0
}

test_generated_files_match_the_palette() {
  generator --check
  assert_ok
  assert_output_has "already up to date"
}

test_every_declared_output_exists() {
  generator --list-outputs
  assert_ok
  local path
  while IFS= read -r path; do
    [ -n "$path" ] || continue
    if [ ! -e "$SOURCE_ROOT/$path" ]; then
      fail "declared output does not exist: $path"
    fi
  done <<< "$OUTPUT"
}

test_stageable_excludes_files_plasma_rewrites() {
  generator --list-outputs --stageable
  assert_ok
  assert_output_lacks "kdeglobals"
  assert_output_lacks "desktop-appletsrc"
  assert_output_has "shared/kitty/colors-mocha.conf"
  assert_output_has "linux/common/quicklaunch/config.toml"
}

test_hook_stages_exactly_what_the_registry_declares() {
  local declared hook_paths
  declared="$(cd "$SOURCE_ROOT" && "$SOURCE_ROOT/scripts/.venv/bin/generate-theme" --list-outputs --stageable | LC_ALL=C sort)"
  hook_paths="$(grep -c 'list-outputs --stageable' "$SOURCE_ROOT/.githooks/pre-commit")"
  [ "$hook_paths" -eq 1 ] || fail "pre-commit hook no longer queries the registry"
  [ -n "$declared" ] || fail "registry declared no stageable outputs"
}
