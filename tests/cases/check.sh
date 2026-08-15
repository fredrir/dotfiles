#!/usr/bin/env bash

check_repo() {
  mkpkg shared/alpha/alpha.conf "alpha"
  mkprofile test shared
}

requires() {
  printf '%s\n' "$@" > "$REPO/config/requirements.dotfile"
}

pins() {
  printf '%s\n' "$@" > "$REPO/config/pins.dotfile"
}

test_passes_when_the_tools_are_in_place() {
  check_repo
  requires "shared {" "  sh" "}"
  dotfile link test
  dotfile check test
  assert_ok
  assert_output_has "check  test"
  assert_output_has "tools      1 installed"
}

test_says_nothing_about_symlinks() {
  check_repo
  requires "shared {" "  sh" "}"
  dotfile check test
  assert_ok
  assert_output_lacks "links"
  assert_output_lacks "~/.config/alpha/alpha.conf"
}

test_reports_a_required_tool_that_is_not_installed() {
  check_repo
  requires "shared {" "  sh" "  definitely-not-installed = some-package" "}"
  dotfile link test
  dotfile check test
  assert_fails
  assert_output_has "tools      1 missing"
  assert_output_has "definitely-not-installed  some-package"
}

test_accepts_a_matching_pin() {
  check_repo
  requires "shared {" "  sh" "}"
  pins "shared {" "  git = git version" "}"
  dotfile link test
  dotfile check test
  assert_ok
  assert_output_has "pins       1 pinned"
}

test_reports_a_pin_mismatch() {
  check_repo
  requires "shared {" "  sh" "}"
  pins "shared {" "  git = not-a-real-build" "}"
  dotfile link test
  dotfile check test
  assert_fails
  assert_output_has "pins       1 mismatched"
  assert_output_has "want not-a-real-build"
}

test_reports_a_pinned_tool_that_is_not_installed() {
  check_repo
  requires "shared {" "  sh" "}"
  pins "shared {" "  definitely-not-installed = 1.0" "}"
  dotfile link test
  dotfile check test
  assert_fails
  assert_output_has "pins       1 mismatched"
  assert_output_has "not installed, want 1.0"
}

test_reports_a_missing_font() {
  check_repo
  requires "shared {" "  font Definitely Not A Font" "}"
  dotfile link test
  dotfile check test
  assert_fails
  assert_output_has "fonts      1 missing"
  assert_output_has "Definitely Not A Font"
}

test_reports_a_missing_file() {
  check_repo
  requires "shared {" "  file ~/.config/nothing-here.png" "}"
  dotfile link test
  dotfile check test
  assert_fails
  assert_output_has "files      1 missing"
  assert_output_has "~/.config/nothing-here.png"
}

test_finds_a_file_that_exists() {
  check_repo
  requires "shared {" "  file ~/.config/something.png" "}"
  : > "$HOME/.config/something.png"
  dotfile link test
  dotfile check test
  assert_ok
  assert_output_has "files      1 installed"
}

test_optional_entries_are_reported_but_never_fail() {
  check_repo
  requires "shared {" "  sh" "  ?definitely-not-installed" "}"
  dotfile link test
  dotfile check test
  assert_ok
  assert_output_has "optional   1 absent"
  assert_output_has "definitely-not-installed"
}

test_reports_zsh_plugins_the_configs_expect() {
  mkprofile test shared
  mkpkg shared/zsh/conf.d/05-ohmyzsh.zsh '[[ -d "$ZSH/custom/plugins/some-plugin" ]] && true'
  requires "shared {" "  sh" "}"
  dotfile link test
  dotfile check test
  assert_fails
  assert_output_has "oh-my-zsh  not installed"
  mkdir -p "$HOME/.oh-my-zsh/custom/plugins"
  dotfile check test
  assert_fails
  assert_output_has "plugins    1 missing"
  assert_output_has "some-plugin"
  mkdir -p "$HOME/.oh-my-zsh/custom/plugins/some-plugin"
  dotfile check test
  assert_ok
  assert_output_has "plugins    1 installed"
}

test_ignores_requirements_for_groups_outside_the_profile() {
  check_repo
  mkpkg macos/beta/beta.conf "beta"
  requires "shared {" "  sh" "}" "" "macos {" "  definitely-not-installed" "}"
  dotfile link test
  dotfile check test
  assert_ok
  assert_output_lacks "definitely-not-installed"
}

test_rejects_a_requirement_for_an_unknown_group() {
  check_repo
  requires "nope {" "  sh" "}"
  dotfile check test
  assert_fails
  assert_output_has "unknown group: nope"
}

test_notes_an_override_that_was_never_selected() {
  check_repo
  mkpkg shared/overrides/laptop/extra.conf "extra"
  requires "shared {" "  sh" "}"
  dotfile check test
  assert_fails
  assert_output_has "overrides  unselected for shared"
}

test_clips_long_lists_until_all_is_passed() {
  check_repo
  tools=("shared {")
  for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14; do
    tools+=("  not-installed-$i")
  done
  tools+=("}")
  requires "${tools[@]}"
  dotfile link test
  dotfile check test
  assert_fails
  assert_output_has "+2 more"
  dotfile check test --all
  assert_fails
  assert_output_lacks "+2 more"
  assert_output_has "not-installed-14"
}

test_uses_the_saved_profile() {
  check_repo
  requires "shared {" "  sh" "}"
  dotfile link test
  dotfile check
  assert_ok
  assert_output_has "check  test"
}
