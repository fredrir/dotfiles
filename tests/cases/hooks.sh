setup_hooks_fixtures() {
  command -v zsh >/dev/null 2>&1 || fail "zsh is required"

  # Trusted-root python project with a fake venv whose activate script
  # behaves like the real one for the parts the hooks touch.
  mkdir -p "$HOME/projects/app/.venv/bin"
  : > "$HOME/projects/app/pyproject.toml"
  printf '%s\n' \
    "export VIRTUAL_ENV=\"$HOME/projects/app/.venv\"" \
    'deactivate() { unset VIRTUAL_ENV; unfunction deactivate; }' \
    > "$HOME/projects/app/.venv/bin/activate"

  # Repo marked by a .git *file* (worktrees and submodules do this).
  mkdir -p "$SANDBOX/tree/repo/a/b"
  : > "$SANDBOX/tree/repo/.git"
}

run_hooks_zsh() {
  local script="$SANDBOX/hooks-test.zsh"
  printf '%s\n' "$1" > "$script"
  SANDBOX="$SANDBOX" \
    HOOKS="$SOURCE_ROOT/shared/zsh/conf.d/95-hooks.zsh" \
    UTILS="$SOURCE_ROOT/shared/zsh/conf.d/90-utils.zsh" \
    zsh -f "$script"
}

test_find_python_project_venv_sets_reply() {
  setup_hooks_fixtures

  run_hooks_zsh '
    cd $HOME
    source $HOOKS
    cd $HOME/projects/app/.venv/bin
    _find_python_project_venv || { print -ru2 -- "finder returned nonzero"; exit 1 }
    want=$HOME/projects/app/.venv; want=${want:A}
    [[ $REPLY == "$want" ]] || { print -ru2 -- "REPLY=$REPLY want=$want"; exit 1 }
  ' || fail "REPLY-based venv finder failed"
}

test_sync_sanitizes_inherited_virtual_env() {
  setup_hooks_fixtures

  # A shell born with a stale VIRTUAL_ENV but no deactivate function must
  # strip the stale bin from path and still activate the wanted venv.
  run_hooks_zsh '
    export VIRTUAL_ENV=$SANDBOX/stale
    mkdir -p $VIRTUAL_ENV/bin
    path=($VIRTUAL_ENV/bin $path)
    cd $HOME/projects/app
    source $HOOKS
    want=$HOME/projects/app/.venv; want=${want:A}
    [[ ${VIRTUAL_ENV:A} == "$want" ]] || { print -ru2 -- "VIRTUAL_ENV=${VIRTUAL_ENV-unset}"; exit 1 }
    (( ${path[(Ie)$SANDBOX/stale/bin]} == 0 )) || { print -ru2 -- "stale bin still in path"; exit 1 }
  ' || fail "inherited VIRTUAL_ENV was not sanitized"
}

test_git_root_walks_to_dot_git() {
  setup_hooks_fixtures

  run_hooks_zsh '
    cd $HOME
    source $HOOKS
    cd $SANDBOX/tree/repo/a/b
    _git_root || { print -ru2 -- "_git_root failed inside repo"; exit 1 }
    want=$SANDBOX/tree/repo; want=${want:A}
    [[ $REPLY == "$want" ]] || { print -ru2 -- "REPLY=$REPLY want=$want"; exit 1 }
    _in_git_repo || { print -ru2 -- "_in_git_repo failed inside repo"; exit 1 }
    [[ ${aliases[gff]-} == "gpp ." ]] || { print -ru2 -- "gff=${aliases[gff]-unset}"; exit 1 }
    cd $HOME
    _git_root && { print -ru2 -- "_git_root succeeded outside repo"; exit 1 }
    [[ -z ${aliases[gff]-} ]] || { print -ru2 -- "gff alias survived leaving repo"; exit 1 }
    exit 0
  ' || fail "_git_root walk misbehaved"
}

test_git_from_root_p_escape_hatch() {
  setup_hooks_fixtures

  TRACE="$SANDBOX/trace"
  COMMANDS="$SANDBOX/commands"
  mkdir -p "$COMMANDS"
  export TRACE
  printf '%s\n' \
    '#!/bin/sh' \
    'printf "git %s\n" "$*" >> "$TRACE"' > "$COMMANDS/git"
  printf '%s\n' \
    '#!/bin/sh' \
    'printf "lazygit %s\n" "$*" >> "$TRACE"' > "$COMMANDS/lazygit"
  chmod +x "$COMMANDS/git" "$COMMANDS/lazygit"

  : > "$TRACE"
  PATH="$COMMANDS:$PATH" run_hooks_zsh '
    cd $HOME
    source $UTILS
    source $HOOKS
    cd $SANDBOX/tree/repo/a/b
    _git_from_root log -p
    _git_from_root log --oneline -p
    _git_from_root log
  ' || fail "_git_from_root run failed"

  assert_file_is "$TRACE" 'git log
git log --oneline -p
lazygit log'
}
