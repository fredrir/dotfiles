#!/usr/bin/env bash

basic_repo() {
  mkpkg shared/alpha/alpha.conf "alpha"
  mkpkg shared/beta/nested/beta.conf "beta"
  mkprofile test shared
}

test_folds_a_package_into_one_symlink() {
  basic_repo
  dotfile link test
  assert_ok
  assert_symlink "$HOME/.config/alpha" "$REPO/shared/alpha"
  assert_symlink "$HOME/.config/beta" "$REPO/shared/beta"
  assert_file_is "$HOME/.config/alpha/alpha.conf" "alpha"
}

test_is_idempotent() {
  basic_repo
  dotfile link test
  dotfile link test
  assert_ok
  assert_symlink "$HOME/.config/alpha" "$REPO/shared/alpha"
  assert_output_lacks "link "
}

test_target_maps_package_contents_into_config_root() {
  mkpkg shared/starship/starship.toml "starship"
  mkprofile test shared
  target "shared/starship = ~/.config"
  dotfile link test
  assert_ok
  assert_realdir "$HOME/.config"
  assert_symlink "$HOME/.config/starship.toml" "$REPO/shared/starship/starship.toml"
  assert_absent "$HOME/.config/starship"
}

test_file_level_target_unfolds_its_package() {
  mkpkg shared/zsh/.zshrc "zshrc"
  mkpkg shared/zsh/conf.d/10-foo.zsh "foo"
  mkprofile test shared
  target "shared/zsh/.zshrc = ~/.zshrc"
  dotfile link test
  assert_ok
  assert_symlink "$HOME/.zshrc" "$REPO/shared/zsh/.zshrc"
  assert_realdir "$HOME/.config/zsh"
  assert_symlink "$HOME/.config/zsh/conf.d" "$REPO/shared/zsh/conf.d"
  assert_absent "$HOME/.config/zsh/.zshrc"
}

test_never_symlinks_a_protected_directory() {
  mkpkg linux/common/theme-watch/gen.service "unit"
  mkprofile test linux/common
  target "linux/common/theme-watch = ~/.config/systemd/user"
  dotfile link test
  assert_ok
  assert_realdir "$HOME/.config/systemd/user"
  assert_symlink "$HOME/.config/systemd/user/gen.service" "$REPO/linux/common/theme-watch/gen.service"
}

test_reports_conflicts_and_exits_nonzero() {
  basic_repo
  printf 'mine\n' > "$HOME/.config/alpha"
  dotfile link test
  assert_fails
  assert_output_has "conflicts"
  assert_output_has "$HOME/.config/alpha"
  assert_file_is "$HOME/.config/alpha" "mine"
}

test_leaves_foreign_symlinks_alone() {
  basic_repo
  mkdir -p "$SANDBOX/elsewhere"
  ln -s "$SANDBOX/elsewhere" "$HOME/.config/alpha"
  dotfile link test
  assert_fails
  assert_output_has "conflicts"
  assert_symlink "$HOME/.config/alpha" "$SANDBOX/elsewhere"
}

test_dry_run_changes_nothing() {
  basic_repo
  dotfile link test -n
  assert_ok
  assert_absent "$HOME/.config/alpha"
  assert_absent "$HOME/.config/dotfile/profile"
  assert_output_has "would:"
}

test_prunes_broken_links_into_the_repo() {
  basic_repo
  ln -s "$REPO/shared/removed/x.conf" "$HOME/.config/removed"
  dotfile link test
  assert_ok
  assert_absent "$HOME/.config/removed"
  assert_output_has "prune"
}

test_keeps_broken_links_that_point_elsewhere() {
  basic_repo
  ln -s "$SANDBOX/not-the-repo" "$HOME/.config/foreign"
  dotfile link test
  assert_ok
  assert_symlink "$HOME/.config/foreign" "$SANDBOX/not-the-repo"
}

test_skips_packages_marked_nolink() {
  mkpkg shared/alpha/alpha.conf "alpha"
  mkpkg shared/skipme/skip.conf "skip"
  mkpkg shared/skipme/.nolink ""
  mkprofile test shared
  dotfile link test
  assert_ok
  assert_symlink "$HOME/.config/alpha" "$REPO/shared/alpha"
  assert_absent "$HOME/.config/skipme"
  assert_output_has ".nolink"
}

test_saves_and_reuses_the_profile() {
  basic_repo
  dotfile link test
  assert_ok
  assert_file_is "$HOME/.config/dotfile/profile" "test"
  rm "$HOME/.config/alpha"
  dotfile link
  assert_ok
  assert_symlink "$HOME/.config/alpha" "$REPO/shared/alpha"
}

test_rejects_an_unknown_profile() {
  basic_repo
  dotfile link nope
  assert_fails
  assert_output_has "no manifest for profile"
  assert_output_has "available profiles"
}
