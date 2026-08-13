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
  assert_output_has "shared/kitty/colors.conf"
  assert_output_has "linux/common/quicklaunch/config.toml"
}

test_list_profiles_marks_the_active_one() {
  generator --list-profiles
  assert_ok
  assert_output_has "mocha"
  assert_output_has "latte"
  local marked
  marked="$(printf '%s\n' "$OUTPUT" | grep -c '(active)')"
  [ "$marked" -eq 1 ] || fail "expected exactly one profile marked active, got $marked"
}

test_switching_profile_is_reported_but_not_written() {
  local before
  before="$(cat "$SOURCE_ROOT/theme/active")"

  generator --profile latte --check
  [ "$STATUS" -ne 0 ] || fail "switching to a different profile should report changes"
  assert_output_has "latte"

  [ "$(cat "$SOURCE_ROOT/theme/active")" = "$before" ] ||
    fail "--check wrote theme/active"
}

test_unknown_profile_is_rejected() {
  generator --profile nope --check
  [ "$STATUS" -ne 0 ] || fail "an unknown profile should fail"
  assert_output_has "unknown profile"
}

test_hook_stages_exactly_what_the_registry_declares() {
  local declared hook_paths
  declared="$(cd "$SOURCE_ROOT" && "$SOURCE_ROOT/scripts/.venv/bin/generate-theme" --list-outputs --stageable | LC_ALL=C sort)"
  hook_paths="$(grep -c 'list-outputs --stageable' "$SOURCE_ROOT/.githooks/pre-commit")"
  [ "$hook_paths" -eq 1 ] || fail "pre-commit hook no longer queries the registry"
  [ -n "$declared" ] || fail "registry declared no stageable outputs"
}
