#!/usr/bin/env bash

# The dmux GUI-side suite owns its own preconditions, so this case is a thin
# handoff. It runs against the real checkout rather than the sandbox repo: the
# tests load shared/wezterm modules by path and assert on source, not on
# anything `dotfile` installs.

test_dmux_gui_suite() {
  command -v lua >/dev/null 2>&1 || fail "lua is required"
  sh "$SOURCE_ROOT/shared/wezterm/wez/dmux_bridge/tests/suite.sh" || fail "dmux GUI suite failed"
}
