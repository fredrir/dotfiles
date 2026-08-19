setup_dotfile_commands() {
  command -v zsh >/dev/null 2>&1 || fail "zsh is required"

  TRACE="$SANDBOX/dotfile-trace"
  COMMANDS="$SANDBOX/commands"
  mkdir -p "$COMMANDS"
  export TRACE DOTFILE_STATUS=0

  printf '%s\n' \
    '#!/bin/sh' \
    'printf "%s\n" "$*" >> "$TRACE"' \
    'exit "$DOTFILE_STATUS"' > "$COMMANDS/dotfile"
  printf '%s\n' \
    '#!/bin/sh' \
    'printf "%s\n" reload >> "$TRACE"' > "$COMMANDS/zsh"
  chmod +x "$COMMANDS/dotfile" "$COMMANDS/zsh"
}

run_dotfile_function() {
  local real_zsh
  real_zsh="$(command -v zsh)"
  PATH="$COMMANDS:$PATH" "$real_zsh" -f -c \
    "source '$SOURCE_ROOT/shared/zsh/conf.d/89-dotfile-sync.zsh'; dotfile $1"
}

test_dotfile_sync_reloads_zsh_after_success() {
  setup_dotfile_commands

  run_dotfile_function sync || fail "dotfile sync failed"

  assert_file_is "$TRACE" 'sync
reload'
}

test_dotfile_sync_does_not_reload_zsh_after_failure() {
  setup_dotfile_commands
  DOTFILE_STATUS=7
  export DOTFILE_STATUS

  if run_dotfile_function sync; then
    fail "failing dotfile sync returned success"
  else
    [ "$?" -eq 7 ] || fail "dotfile sync did not preserve failure status"
  fi

  assert_file_is "$TRACE" 'sync'
}

test_other_dotfile_commands_do_not_reload_zsh() {
  setup_dotfile_commands

  run_dotfile_function status || fail "dotfile status failed"

  assert_file_is "$TRACE" 'status'
}

setup_zshenv() {
  command -v zsh >/dev/null 2>&1 || fail "zsh is required"

  ZSH_BIN="$(command -v zsh)"
  ZDOTDIR="$SANDBOX/zdotdir"
  mkdir -p "$ZDOTDIR"
  # A copy, not a link: the point is what a real zsh picks up on its own, and
  # `zsh -f` would skip .zshenv along with everything else.
  cp "$SOURCE_ROOT/shared/zsh/.zshenv" "$ZDOTDIR/.zshenv"
  export ZDOTDIR
}

count_local_bin() {
  "$ZSH_BIN" -c "$1" | grep -c -x "$HOME/.local/bin"
}

test_zshenv_puts_local_bin_on_the_non_interactive_path() {
  setup_zshenv

  local count
  count="$(count_local_bin 'print -rl -- $path')"
  [ "$count" = "1" ] || fail "expected one ~/.local/bin in \$path, got $count"
}

test_zshenv_does_not_duplicate_when_conf_d_prepends_again() {
  setup_zshenv

  local count
  count="$(count_local_bin 'path=("$HOME/.local/bin" $path); print -rl -- $path')"
  [ "$count" = "1" ] || fail "conf.d prepend duplicated ~/.local/bin ($count entries)"
}

test_zshenv_is_silent() {
  setup_zshenv

  local output
  output="$("$ZSH_BIN" -c true 2>&1)"
  [ -z "$output" ] || fail "zshenv wrote output:
$output"
}
