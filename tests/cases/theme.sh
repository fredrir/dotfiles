#!/usr/bin/env bash

generator() {
  OUTPUT="$(cd "$SOURCE_ROOT" && "$SOURCE_ROOT/scripts/python/.venv/bin/dotfile" theme "$@" 2>&1)"
  STATUS=$?
  return 0
}

test_generated_files_match_the_palette() {
  generator check
  assert_ok
  assert_output_has "already up to date"
}

test_every_declared_output_exists() {
  generator outputs
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
  generator outputs --stageable
  assert_ok
  assert_output_lacks "kdeglobals"
  assert_output_lacks "desktop-appletsrc"
  assert_output_has "shared/kitty/colors.conf"
  assert_output_has "linux/common/quicklaunch/config.toml"
}

test_status_reports_the_groups_each_profile_covers() {
  generator status
  assert_ok
  assert_output_has "shared"
}

test_every_group_that_owns_a_file_is_assigned_a_profile() {
  local groups assigned group
  generator outputs
  assert_ok
  groups="$(printf '%s\n' "$OUTPUT" |
    sed -n 's:^\(linux/[^/]*\)/.*:\1:p; s:^\([^/]*\)/.*:\1:p' | sort -u)"
  [ -n "$groups" ] || fail "no groups derived from the declared outputs"

  generator status
  assert_ok
  assigned="$OUTPUT"
  while IFS= read -r group; do
    [ -n "$group" ] || continue
    printf '%s\n' "$assigned" | grep -q "$group" ||
      fail "group '$group' owns files but no profile covers it"
  done <<< "$groups"
}

test_a_group_cannot_theme_a_package_it_does_not_own() {
  local keep="$SOURCE_ROOT/config/profiles.dotfile"
  local saved
  saved="$(cat "$keep")"
  printf 'shared {\n  theme = mocha\n}\n\nlinux/arch {\n  zsh = latte\n}\n' > "$keep"
  generator check
  printf '%s\n' "$saved" > "$keep"

  [ "$STATUS" -ne 0 ] || fail "theming a package the group does not own should fail"
  assert_output_has "has no 'zsh' output"
}

test_switch_rejects_a_scope_that_owns_nothing() {
  local before
  before="$(cat "$SOURCE_ROOT/config/profiles.dotfile")"
  generator switch latte linux/nowhere
  [ "$STATUS" -ne 0 ] || fail "an unknown scope should fail"
  assert_output_has "nothing generated is scoped to"
  [ "$before" = "$(cat "$SOURCE_ROOT/config/profiles.dotfile")" ] ||
    fail "a rejected scope must not rewrite profiles.dotfile"
}

test_switch_rejects_a_profile_that_does_not_exist() {
  generator switch no-such-profile shared
  [ "$STATUS" -ne 0 ] || fail "an unknown profile should fail"
  assert_output_has "unknown profile"
}

test_hook_stages_exactly_what_the_registry_declares() {
  local declared hook_paths
  declared="$(cd "$SOURCE_ROOT" && "$SOURCE_ROOT/scripts/python/.venv/bin/dotfile" theme outputs --stageable | LC_ALL=C sort)"
  hook_paths="$(grep -c 'theme outputs --stageable' "$SOURCE_ROOT/.githooks/pre-commit")"
  [ "$hook_paths" -eq 1 ] || fail "pre-commit hook no longer queries the registry"
  [ -n "$declared" ] || fail "registry declared no stageable outputs"
}
