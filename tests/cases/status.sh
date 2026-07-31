#!/usr/bin/env bash

status_repo() {
  mkpkg shared/alpha/alpha.conf "alpha"
  mkpkg shared/beta/beta.conf "beta"
  mkprofile test shared
}

test_counts_everything_as_linked_after_a_link() {
  status_repo
  dotfile link test
  dotfile status test
  assert_ok
  assert_output_has "2 linked, 0 missing, 0 differing"
}

test_reports_missing_destinations() {
  status_repo
  dotfile status test
  assert_fails
  assert_output_has "0 linked, 2 missing, 0 differing"
  assert_output_has "missing"
}

test_reports_destinations_owned_by_something_else() {
  status_repo
  dotfile link test
  rm "$HOME/.config/alpha"
  mkdir -p "$HOME/.config/alpha"
  printf 'mine\n' > "$HOME/.config/alpha/alpha.conf"
  dotfile status test
  assert_fails
  assert_output_has "differs"
  assert_output_has "1 linked, 0 missing, 1 differing"
}

test_uses_the_saved_profile() {
  status_repo
  dotfile link test
  dotfile status
  assert_ok
  assert_output_has "profile 'test'"
}
