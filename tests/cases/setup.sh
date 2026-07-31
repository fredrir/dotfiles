#!/usr/bin/env bash

source "$SOURCE_ROOT/scripts/lib/profiles.sh"

setup_profiles() {
  ENVDIR="$REPO/environment"
  mkprofile arch-linux/kde shared linux/common linux/kde
  mkprofile arch-linux/hyprland shared linux/common linux/hyprland
  mkprofile arch-linux/kde-hyprland shared linux/common linux/kde linux/hyprland
  mkprofile macos shared macos
  mkprofile ubuntu/server shared linux/server
}

test_filters_for_arch_with_kde_only() {
  setup_profiles
  local got
  got="$(filter_profiles arch-linux kde)"
  [ "$got" = arch-linux/kde ] || fail "unexpected profiles: $got"
}

test_includes_combined_profile_when_both_desktops_are_installed() {
  setup_profiles
  local got want
  got="$(filter_profiles arch-linux 'kde hyprland')"
  want=$'arch-linux/hyprland\narch-linux/kde\narch-linux/kde-hyprland'
  [ "$got" = "$want" ] || fail "unexpected profiles: $got"
}

test_filters_profiles_by_operating_system() {
  setup_profiles
  local mac ubuntu
  mac="$(filter_profiles macos '')"
  ubuntu="$(filter_profiles ubuntu '')"
  [ "$mac" = macos ] || fail "unexpected macOS profiles: $mac"
  [ "$ubuntu" = ubuntu/server ] || fail "unexpected Ubuntu profiles: $ubuntu"
}

test_normalizes_explicit_environment_override() {
  local got
  got="$(normalize_profile_arg --arch-linux/hyprland)"
  [ "$got" = arch-linux/hyprland ] || fail "unexpected profile: $got"
  got="$(normalize_profile_arg arch-linux/kde)"
  [ "$got" = arch-linux/kde ] || fail "unexpected profile: $got"
}
