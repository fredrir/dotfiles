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

setup_agent_zshrc() {
  command -v zsh >/dev/null 2>&1 || fail "zsh is required"

  ZSH_BIN="$(command -v zsh)"
  ZCONF="$HOME/.config/zsh/conf.d"
  mkdir -p "$ZCONF"

  # Stand-ins for the real modules: each announces itself and does nothing
  # else, so which layers loaded is visible in the output.
  local name
  for name in 03-theme 05-ohmyzsh 10-env 11-env.macos 15-history \
    21-paths.macos 40-aliases 42-agents 55-completions 87-starship \
    88-direnv 90-utils 95-hooks; do
    printf 'print -r -- %s\n' "$name" > "$ZCONF/$name.zsh"
  done

  # Sourced by both branches, and the tail of .zshrc: without it that last
  # line is a false conditional and .zshrc returns non-zero.
  printf 'print -r -- local-bin-env\n' > "$HOME/.local/bin/env"
}

# -f so the sandbox zsh does not read the real dotfiles on the way in; .zshrc
# is then sourced explicitly, which is what is under test. The markers are
# cleared first because these tests may themselves be run from inside an agent.
load_zshrc() {
  env -u AGENT_SHELL -u CLAUDECODE -u AI_AGENT "$@" \
    "$ZSH_BIN" -f -c "source '$SOURCE_ROOT/shared/zsh/.zshrc'"
}

test_agent_shell_loads_the_machine_layer_only() {
  setup_agent_zshrc

  local loaded
  loaded="$(load_zshrc AGENT_SHELL=1)" || fail "sourcing .zshrc failed"

  [ "$loaded" = '10-env
11-env.macos
15-history
21-paths.macos
local-bin-env' ] || fail "agent shell loaded the wrong layer:
$loaded"
}

test_agent_shell_skips_aliases_hooks_and_plugins() {
  setup_agent_zshrc

  local loaded name
  loaded="$(load_zshrc AGENT_SHELL=1)" || fail "sourcing .zshrc failed"

  for name in 03-theme 05-ohmyzsh 40-aliases 42-agents 55-completions \
    87-starship 88-direnv 90-utils 95-hooks; do
    case "$loaded" in
      *"$name"*) fail "$name leaked into an agent shell" ;;
    esac
  done
}

test_vendor_marker_alone_marks_an_agent_shell() {
  setup_agent_zshrc

  local loaded
  loaded="$(load_zshrc CLAUDECODE=1)" || fail "sourcing .zshrc failed"

  case "$loaded" in
    *90-utils*) fail "CLAUDECODE=1 did not mark the shell as an agent" ;;
  esac
}

test_shell_without_a_marker_still_loads_everything() {
  setup_agent_zshrc

  local loaded name
  loaded="$(load_zshrc)" || fail "sourcing .zshrc failed"

  for name in 03-theme 05-ohmyzsh 10-env 40-aliases 42-agents 88-direnv \
    90-utils 95-hooks local-bin-env; do
    case "$loaded" in
      *"$name"*) ;;
      *) fail "$name did not load in a human shell:
$loaded" ;;
    esac
  done
}

setup_agent_commands() {
  command -v zsh >/dev/null 2>&1 || fail "zsh is required"

  ZSH_BIN="$(command -v zsh)"
  TRACE="$SANDBOX/agent-trace"
  COMMANDS="$SANDBOX/commands"
  mkdir -p "$COMMANDS"
  export TRACE

  local name
  for name in claude codex opencode; do
    printf '%s\n' \
      '#!/bin/sh' \
      "printf '%s %s %s\\n' $name \"\${AGENT_SHELL:-unset}\" \"\$*\" >> \"\$TRACE\"" \
      > "$COMMANDS/$name"
    chmod +x "$COMMANDS/$name"
  done
}

run_agent_wrapper() {
  PATH="$COMMANDS:$PATH" "$ZSH_BIN" -f -c \
    "source '$SOURCE_ROOT/shared/zsh/conf.d/42-agents.zsh'; $1"
}

test_agent_wrappers_mark_the_command_and_keep_their_flags() {
  setup_agent_commands

  run_agent_wrapper 'claude -p hello' || fail "claude wrapper failed"
  run_agent_wrapper 'codex exec build' || fail "codex wrapper failed"
  run_agent_wrapper 'opencode run' || fail "opencode wrapper failed"

  assert_file_is "$TRACE" 'claude 1 --dangerously-skip-permissions -p hello
codex 1 --yolo exec build
opencode 1 run'
}

test_agent_wrappers_do_not_mark_the_shell_they_were_run_from() {
  setup_agent_commands

  local after
  after="$(run_agent_wrapper 'claude x; print -r -- "marker=${AGENT_SHELL:-unset}"')" \
    || fail "claude wrapper failed"

  [ "$after" = "marker=unset" ] || fail "wrapper leaked the marker into its caller: $after"
}
