autoload -Uz add-zsh-hook

typeset -ga PYTHON_VENV_AUTO_ROOTS=(
  "$HOME/projects"
  "$HOME/sndbx"
  "$HOME/llunde-new"
)

_find_python_project_venv() {
  local current_dir=${PWD:A}
  local trusted_root search_dir

  for trusted_root in "${PYTHON_VENV_AUTO_ROOTS[@]}"; do
    [[ -d $trusted_root ]] || continue
    trusted_root=${trusted_root:A}

    [[ $current_dir == $trusted_root || $current_dir == $trusted_root/* ]] || continue
    search_dir=$current_dir

    while true; do
      if [[ -f "$search_dir/pyproject.toml" && -f "$search_dir/.venv/bin/activate" ]]; then
        print -r -- "$search_dir/.venv"
        return 0
      fi

      [[ $search_dir == $trusted_root ]] && break
      search_dir=${search_dir:h}
    done
  done

  return 1
}

_sync_python_project_venv() {
  local wanted_venv=""
  local active_venv=${VIRTUAL_ENV:-}

  wanted_venv=$(_find_python_project_venv) || wanted_venv=""
  [[ -n $wanted_venv ]] && wanted_venv=${wanted_venv:A}
  [[ -n $active_venv ]] && active_venv=${active_venv:A}

  [[ $active_venv == $wanted_venv ]] && return 0

  if [[ -n $active_venv ]]; then
    (( $+functions[deactivate] )) || return 0
    deactivate
  fi

  [[ -n $wanted_venv ]] || return 0
  source "$wanted_venv/bin/activate"
}

add-zsh-hook -d chpwd _auto_deactivate_project_venv 2>/dev/null
unfunction _auto_deactivate_project_venv 2>/dev/null

add-zsh-hook -d chpwd _sync_python_project_venv 2>/dev/null
add-zsh-hook chpwd _sync_python_project_venv
_sync_python_project_venv

_cdg_to_root() {
  local root

  root=$(command git rev-parse --show-toplevel 2>/dev/null) || return 0
  root=${root:A}

  [[ ${PWD:A} == $root ]] && return 0
  builtin cd -- "$root"
}

_git_from_root() {
  local root

  root=$(command git rev-parse --show-toplevel 2>/dev/null) || return 1

  if [[ $2 == -p ]]; then
    (builtin cd -- "$root" && command git "$@")
    return
  fi

  (builtin cd -- "$root" && git "$@")
}

_git_finish_from_root() {
  _git_from_root add . &&
    _git_from_root commit -m "." &&
    _git_from_root push
}

_sync_git_repo_commands() {
  if command git rev-parse --show-toplevel >/dev/null 2>&1; then
    alias cdg=_cdg_to_root
    alias gs='_git_from_root status'
    alias ga='_git_from_root add .'
    alias gc='_git_from_root commit -m'
    alias gcm='_git_from_root commit -m'
    alias gp='_git_from_root push'
    alias gl='_git_from_root log'
    alias gd='_git_from_root diff'
    alias gff=_git_finish_from_root
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
