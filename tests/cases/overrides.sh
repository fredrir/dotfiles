#!/usr/bin/env bash

override_repo() {
  mkpkg linux/hyprland/hypr/hyprland.conf "base"
  mkpkg linux/hyprland/overrides/laptop/hypr-local/local.conf "laptop"
  mkpkg linux/hyprland/overrides/desktop/hypr-local/local.conf "desktop"
  mkprofile test linux/hyprland
}

test_notes_unselected_overrides_without_guessing() {
  override_repo
  dotfile link test
  assert_ok
  assert_output_has "has machine overrides"
  assert_output_has "desktop laptop"
  assert_symlink "$HOME/.config/hypr" "$REPO/linux/hyprland/hypr"
  assert_absent "$HOME/.config/hypr-local"
}

test_links_the_selected_override() {
  override_repo
  dotfile link test --override linux/hyprland=laptop
  assert_ok
  assert_symlink "$HOME/.config/hypr-local" "$REPO/linux/hyprland/overrides/laptop/hypr-local"
  assert_file_is "$HOME/.config/hypr-local/local.conf" "laptop"
}

test_remembers_the_selection() {
  override_repo
  dotfile link test --override linux/hyprland=laptop
  assert_file_is "$HOME/.config/dotfile/overrides" "linux/hyprland=laptop"
  dotfile link test
  assert_ok
  assert_output_lacks "has machine overrides"
  assert_symlink "$HOME/.config/hypr-local" "$REPO/linux/hyprland/overrides/laptop/hypr-local"
}

test_prunes_the_previous_override_when_switching() {
  override_repo
  dotfile link test --override linux/hyprland=laptop
  dotfile link test --override linux/hyprland=desktop
  assert_ok
  assert_symlink "$HOME/.config/hypr-local" "$REPO/linux/hyprland/overrides/desktop/hypr-local"
  assert_file_is "$HOME/.config/hypr-local/local.conf" "desktop"
}

test_none_opts_out_and_prunes() {
  override_repo
  dotfile link test --override linux/hyprland=laptop
  dotfile link test --override linux/hyprland=none
  assert_ok
  assert_absent "$HOME/.config/hypr-local"
  assert_symlink "$HOME/.config/hypr" "$REPO/linux/hyprland/hypr"
}

test_rejects_an_unknown_override_name() {
  override_repo
  dotfile link test --override linux/hyprland=nope
  assert_fails
  assert_output_has "unknown override"
}

test_rejects_overrides_for_a_group_without_any() {
  override_repo
  dotfile link test --override shared=laptop
  assert_fails
  assert_output_has "no overrides"
}
