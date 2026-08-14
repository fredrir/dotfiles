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
