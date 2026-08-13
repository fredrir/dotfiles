#!/usr/bin/env bash

test_adopts_a_config_directory() {
  mkdir -p "$HOME/.config/waybar"
  printf 'bar\n' > "$HOME/.config/waybar/config"
  dotfile add waybar
  assert_ok
  assert_file_is "$REPO/shared/waybar/config" "bar"
  assert_symlink "$HOME/.config/waybar" "$REPO/shared/waybar"
}

test_adopts_a_loose_config_file_and_maps_it() {
  printf 'x\n' > "$HOME/.config/foot.ini"
  dotfile add foot.ini
  assert_ok
  assert_file_is "$REPO/shared/foot/foot.ini" "x"
  assert_symlink "$HOME/.config/foot.ini" "$REPO/shared/foot/foot.ini"
  assert_output_has "shared/foot = ~/.config"
  grep -qxF "shared/foot = ~/.config" "$REPO/targets" || fail "targets entry not written"
}

test_places_into_the_requested_group() {
  mkdir -p "$HOME/.config/waybar"
  printf 'bar\n' > "$HOME/.config/waybar/config"
  dotfile add --hyprland waybar
  assert_ok
  assert_file_is "$REPO/linux/hyprland/waybar/config" "bar"
  assert_symlink "$HOME/.config/waybar" "$REPO/linux/hyprland/waybar"
}

test_does_not_warn_when_the_group_is_in_the_profile() {
  mkdir -p "$HOME/.config/waybar" "$HOME/.config/dotfile"
  printf 'bar\n' > "$HOME/.config/waybar/config"
  printf 'test\n' > "$HOME/.config/dotfile/profile"
  mkprofile test shared linux/common linux/kde

  dotfile add --linux waybar
  assert_ok
  assert_output_lacks "is not in environment/test/manifest"
}

test_warns_when_the_group_is_not_in_the_profile() {
  mkdir -p "$HOME/.config/waybar" "$HOME/.config/dotfile"
  printf 'bar\n' > "$HOME/.config/waybar/config"
  printf 'test\n' > "$HOME/.config/dotfile/profile"
  mkprofile test shared

  dotfile add --linux waybar
  assert_ok
  assert_output_has "group 'linux/common' is not in environment/test/manifest"
}

test_honours_an_explicit_package_name() {
  printf 'rc\n' > "$HOME/.zshrc"
  dotfile add --pkg zsh "$HOME/.zshrc"
  assert_ok
  assert_file_is "$REPO/shared/zsh/.zshrc" "rc"
  assert_symlink "$HOME/.zshrc" "$REPO/shared/zsh/.zshrc"
  grep -qxF "shared/zsh/.zshrc = ~/.zshrc" "$REPO/targets" || fail "targets entry not written"
}

test_keeps_subdirectories_when_the_package_names_the_dotdir() {
  mkdir -p "$HOME/.ssh/config.d"
  printf 'cabled\n' > "$HOME/.ssh/config.d/40-cabled"
  dotfile add --pkg ssh "$HOME/.ssh/config.d/40-cabled"
  assert_ok
  assert_file_is "$REPO/shared/ssh/config.d/40-cabled" "cabled"
  assert_symlink "$HOME/.ssh/config.d/40-cabled" "$REPO/shared/ssh/config.d/40-cabled"
  grep -qxF "shared/ssh = ~/.ssh" "$REPO/targets" || fail "directory targets entry not written"
}

test_flattens_when_the_package_does_not_name_the_dotdir() {
  mkdir -p "$HOME/.ssh"
  printf 'signers\n' > "$HOME/.ssh/allowed"
  dotfile add --pkg git "$HOME/.ssh/allowed"
  assert_ok
  assert_file_is "$REPO/shared/git/allowed" "signers"
  grep -qxF "shared/git/allowed = ~/.ssh/allowed" "$REPO/targets" || fail "file targets entry not written"
}

test_requires_a_package_name_outside_config() {
  printf 'rc\n' > "$HOME/.zshrc"
  dotfile add "$HOME/.zshrc"
  assert_fails
  assert_output_has "need --pkg"
  assert_file_is "$HOME/.zshrc" "rc"
}

test_records_a_description() {
  mkdir -p "$HOME/.config/waybar"
  printf 'bar\n' > "$HOME/.config/waybar/config"
  dotfile add --description "Status bar" waybar
  assert_ok
  grep -qF "waybar = Status bar" "$REPO/packages.dotfile" || fail "description not recorded"
  grep -qF "\`waybar\` — Status bar" "$REPO/PACKAGES.md" || fail "description not documented"
}

test_refuses_an_already_managed_path() {
  mkpkg shared/waybar/config "bar"
  ln -s "$REPO/shared/waybar" "$HOME/.config/waybar"
  dotfile add waybar
  assert_fails
  assert_output_has "already managed"
}

test_refuses_a_foreign_symlink() {
  mkdir -p "$SANDBOX/elsewhere"
  ln -s "$SANDBOX/elsewhere" "$HOME/.config/waybar"
  dotfile add waybar
  assert_fails
  assert_output_has "foreign symlink"
  assert_symlink "$HOME/.config/waybar" "$SANDBOX/elsewhere"
}

test_refuses_when_the_destination_exists() {
  mkpkg shared/waybar/config "tracked"
  mkdir -p "$HOME/.config/waybar"
  printf 'live\n' > "$HOME/.config/waybar/config"
  dotfile add waybar
  assert_fails
  assert_output_has "exists"
  assert_file_is "$HOME/.config/waybar/config" "live"
}

test_refuses_a_partially_managed_directory() {
  mkpkg shared/other/thing.conf "thing"
  mkdir -p "$HOME/.config/waybar"
  printf 'bar\n' > "$HOME/.config/waybar/config"
  ln -s "$REPO/shared/other/thing.conf" "$HOME/.config/waybar/linked.conf"
  dotfile add waybar
  assert_fails
  assert_output_has "already partially managed"
}

test_refuses_a_path_that_does_not_exist() {
  dotfile add nothing-here
  assert_fails
  assert_output_has "not found"
}
