#!/usr/bin/env bash

test_removes_a_package_and_keeps_it_live() {
  mkpkg linux/common/fontconfig/fonts.conf "fonts"
  mkprofile test linux/common
  dotfile link test
  assert_symlink "$HOME/.config/fontconfig" "$REPO/linux/common/fontconfig"

  dotfile remove linux/common/fontconfig
  assert_ok
  assert_absent "$REPO/linux/common/fontconfig"
  assert_realdir "$HOME/.config/fontconfig"
  assert_file_is "$HOME/.config/fontconfig/fonts.conf" "fonts"
  assert_output_has "removed linux/common/fontconfig"
}

test_removes_only_the_requested_file_from_a_folded_package() {
  mkpkg linux/server/zsh/conf.d/10-nvim.server.zsh "nvim"
  mkpkg linux/server/zsh/conf.d/20-paths.server.zsh "paths"
  mkprofile test linux/server
  dotfile link test
  assert_symlink "$HOME/.config/zsh" "$REPO/linux/server/zsh"

  dotfile remove /linux/server/zsh/conf.d/10-nvim.server.zsh
  assert_ok
  assert_absent "$REPO/linux/server/zsh/conf.d/10-nvim.server.zsh"
  assert_file_is "$REPO/linux/server/zsh/conf.d/20-paths.server.zsh" "paths"
  assert_realdir "$HOME/.config/zsh"
  assert_realdir "$HOME/.config/zsh/conf.d"
  [ ! -L "$HOME/.config/zsh/conf.d/10-nvim.server.zsh" ] || fail "removed file is still managed"
  assert_file_is "$HOME/.config/zsh/conf.d/10-nvim.server.zsh" "nvim"
  assert_symlink "$HOME/.config/zsh/conf.d/20-paths.server.zsh" "$REPO/linux/server/zsh/conf.d/20-paths.server.zsh"
}

test_removes_only_the_requested_directory() {
  mkpkg shared/zsh/conf.d/10-nvim.zsh "nvim"
  mkpkg shared/zsh/.zshrc "zshrc"
  mkprofile test shared
  dotfile link test

  dotfile remove shared/zsh/conf.d
  assert_ok
  assert_absent "$REPO/shared/zsh/conf.d"
  assert_file_is "$REPO/shared/zsh/.zshrc" "zshrc"
  assert_realdir "$HOME/.config/zsh"
  assert_realdir "$HOME/.config/zsh/conf.d"
  assert_file_is "$HOME/.config/zsh/conf.d/10-nvim.zsh" "nvim"
  assert_symlink "$HOME/.config/zsh/.zshrc" "$REPO/shared/zsh/.zshrc"
}

test_removes_only_target_entries_below_the_requested_path() {
  mkpkg shared/zsh/.zshrc "zshrc"
  mkpkg shared/zsh/conf.d/10-paths.zsh "paths"
  mkprofile test shared
  target "shared/zsh/.zshrc = ~/.zshrc"
  target "shared/zsh/conf.d = ~/.config/zsh/conf.d"
  dotfile link test

  dotfile remove shared/zsh/.zshrc
  assert_ok
  assert_file_is "$HOME/.zshrc" "zshrc"
  grep -qxF "shared/zsh/conf.d = ~/.config/zsh/conf.d" "$REPO/targets" || fail "sibling target was removed"
  if grep -qF "shared/zsh/.zshrc" "$REPO/targets"; then fail "removed target remains"; fi
}

test_honours_a_target_added_after_a_package_was_folded() {
  mkpkg shared/zsh/.zshrc "zshrc"
  mkpkg shared/zsh/conf.d/10-paths.zsh "paths"
  mkprofile test shared
  dotfile link test
  target "shared/zsh/.zshrc = ~/.zshrc"

  dotfile remove shared/zsh
  assert_ok
  assert_absent "$REPO/shared/zsh"
  assert_file_is "$HOME/.zshrc" "zshrc"
  assert_file_is "$HOME/.config/zsh/conf.d/10-paths.zsh" "paths"
}

test_keeps_an_existing_live_config() {
  mkpkg shared/alpha/config "tracked"
  mkpkg shared/alpha/extra "extra"
  mkdir -p "$HOME/.config/alpha"
  printf 'live\n' > "$HOME/.config/alpha/config"

  dotfile remove shared/alpha
  assert_ok
  assert_absent "$REPO/shared/alpha"
  assert_file_is "$HOME/.config/alpha/config" "live"
  assert_file_is "$HOME/.config/alpha/extra" "extra"
  assert_output_has "kept   existing ~/.config/alpha/config"
}

test_keeps_a_live_path_that_blocks_the_tracked_structure() {
  mkpkg shared/alpha/config "tracked"
  printf 'live\n' > "$HOME/.config/alpha"

  dotfile remove shared/alpha/config
  assert_ok
  assert_absent "$REPO/shared/alpha"
  assert_file_is "$HOME/.config/alpha" "live"
}

test_rejects_a_path_outside_a_package() {
  dotfile remove scripts/dotfile
  assert_fails
  assert_output_has "not a package path"
}

test_rejects_a_missing_path() {
  mkdir -p "$REPO/shared/alpha"
  dotfile remove shared/alpha/missing
  assert_fails
  assert_output_has "not found in dotfiles"
}
