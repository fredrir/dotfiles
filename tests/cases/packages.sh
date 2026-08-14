#!/usr/bin/env bash

packages_repo() {
  mkpkg shared/alpha/alpha.conf "a"
  mkpkg shared/beta/beta.conf "b"
  mkpkg linux/kde/gamma/gamma.conf "g"
  mkprofile test shared linux/kde
}

test_generates_the_manifest_from_the_tree() {
  packages_repo
  dotfile packages
  assert_ok
  assert_file_is "$REPO/config/packages.dotfile" "shared {
  alpha
  beta
}

linux/kde {
  gamma
}"
}

test_generates_the_documentation() {
  packages_repo
  dotfile packages
  assert_ok
  assert_file_is "$REPO/PACKAGES.md" "
## \`shared\`

- \`alpha\`
- \`beta\`

## \`linux/kde\`

- \`gamma\`"
}

test_preserves_descriptions_across_regeneration() {
  packages_repo
  dotfile packages
  printf 'shared {\n  alpha = First one\n  beta\n}\n\nlinux/kde {\n  gamma\n}\n' > "$REPO/config/packages.dotfile"
  mkpkg shared/delta/delta.conf "d"
  dotfile packages
  assert_ok
  grep -qF "alpha = First one" "$REPO/config/packages.dotfile" || fail "description lost"
  grep -qF "delta" "$REPO/config/packages.dotfile" || fail "new package missing"
  grep -qF "\`alpha\` — First one" "$REPO/PACKAGES.md" || fail "description not documented"
}

test_ignores_override_directories() {
  packages_repo
  mkpkg linux/kde/overrides/laptop/thing/x.conf "x"
  dotfile packages
  assert_ok
  assert_output_lacks "overrides"
  grep -qF "overrides" "$REPO/config/packages.dotfile" && fail "overrides leaked into the manifest"
  return 0
}

test_is_quiet_when_already_current() {
  packages_repo
  dotfile packages
  dotfile packages
  assert_ok
  assert_output_has "packages are current"
}

test_rejects_a_duplicate_entry() {
  packages_repo
  printf 'shared {\n  alpha\n  alpha\n}\n' > "$REPO/config/packages.dotfile"
  dotfile packages
  assert_fails
  assert_output_has "duplicate package"
}

test_rejects_a_package_outside_a_group() {
  packages_repo
  printf 'alpha\n' > "$REPO/config/packages.dotfile"
  dotfile packages
  assert_fails
  assert_output_has "outside a group"
}

test_rejects_an_unclosed_group() {
  packages_repo
  printf 'shared {\n  alpha\n' > "$REPO/config/packages.dotfile"
  dotfile packages
  assert_fails
  assert_output_has "missing }"
}
