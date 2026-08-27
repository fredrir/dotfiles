autoload -Uz add-zsh-hook

typeset -ga PYTHON_VENV_AUTO_ROOTS=(
  "$HOME/projects"
  "$HOME/sndbx"
  "$HOME/llunde-new"
)

# Leaves the found venv in REPLY (no subshell — this runs on every chpwd).
_find_python_project_venv() {
  local current_dir=${PWD:A}
  local trusted_root search_dir
  typeset -g REPLY=

  for trusted_root in "${PYTHON_VENV_AUTO_ROOTS[@]}"; do
    [[ -d $trusted_root ]] || continue
    trusted_root=${trusted_root:A}

    [[ $current_dir == "$trusted_root" || $current_dir == "$trusted_root"/* ]] || continue
    search_dir=$current_dir

    while true; do
      if [[ -f "$search_dir/pyproject.toml" && -f "$search_dir/.venv/bin/activate" ]]; then
        REPLY=$search_dir/.venv
        return 0
      fi

      [[ $search_dir == "$trusted_root" ]] && break
      search_dir=${search_dir:h}
    done
  done

  return 1
}

_sync_python_project_venv() {
  local wanted_venv=""
  local active_venv=${VIRTUAL_ENV:-}

  _find_python_project_venv && wanted_venv=$REPLY
  [[ -n $wanted_venv ]] && wanted_venv=${wanted_venv:A}
  [[ -n $active_venv ]] && active_venv=${active_venv:A}

  [[ $active_venv == "$wanted_venv" ]] && return 0

  if [[ -n $active_venv ]]; then
    if (($+functions[deactivate])); then
      deactivate
    else
      # VIRTUAL_ENV inherited from a parent shell: no deactivate to call,
      # so sanitize by hand and carry on with the sync.
      path=(${path:#"$VIRTUAL_ENV"/bin})
      unset VIRTUAL_ENV
    fi
  fi

  [[ -n $wanted_venv ]] || return 0
  source "$wanted_venv/bin/activate"
}

add-zsh-hook -d chpwd _auto_deactivate_project_venv 2>/dev/null
unfunction _auto_deactivate_project_venv 2>/dev/null

add-zsh-hook -d chpwd _sync_python_project_venv 2>/dev/null
add-zsh-hook chpwd _sync_python_project_venv
_sync_python_project_venv

# Fork-free stand-in for `git rev-parse --show-toplevel`: walk up looking
# for a .git entry, leaving the root in REPLY. -e, not -d — worktrees and
# submodules keep their .git as a file. Diverges from rev-parse only when
# inside .git itself, which is acceptable here.
_git_root() {
  local dir=${PWD:A}

  while true; do
    if [[ -e $dir/.git ]]; then
      typeset -g REPLY=$dir
      return 0
    fi
    [[ $dir == / ]] && return 1
    dir=${dir:h}
  done
}

_in_git_repo() {
  _git_root
}

_cdg_to_root() {
  _git_root || return 0

  [[ ${PWD:A} == "$REPLY" ]] && return 0
  builtin cd -- "$REPLY"
}

# A `-p` that is the sole argument after the subcommand is the "plain git"
# escape hatch: `gl -p` runs `command git log`, skipping the lazygit wrapper
# (which only fires on a bare subcommand anyway). The sentinel is consumed,
# never handed to git; a `-p` accompanied by other arguments is git's own
# patch flag and passes through untouched.
_git_from_root() {
  _git_root || return 1
  local root=$REPLY

  if (($# == 2)) && [[ $2 == -p ]]; then
    (builtin cd -- "$root" && command git "$1")
    return
  fi

  (builtin cd -- "$root" && git "$@")
}

_sync_git_repo_commands() {
  if _in_git_repo; then
    alias cdg=_cdg_to_root
    alias gs='_git_from_root status'
    alias ga='_git_from_root add .'
    alias gc='_git_from_root commit -m'
    alias gcm='_git_from_root commit -m'
    alias gp='_git_from_root push'
    alias gl='_git_from_root log'
    alias gd='_git_from_root diff'
    alias gff='gpp .'
  else
    unalias cdg gs ga gc gcm gp gl gd gff 2>/dev/null
  fi
}

unalias cdg gs ga gc gcm gp gl gd gff 2>/dev/null
add-zsh-hook -d chpwd _sync_cdg_command 2>/dev/null
unfunction _sync_cdg_command 2>/dev/null
add-zsh-hook -d chpwd _sync_git_repo_commands 2>/dev/null
add-zsh-hook chpwd _sync_git_repo_commands
_sync_git_repo_commands
