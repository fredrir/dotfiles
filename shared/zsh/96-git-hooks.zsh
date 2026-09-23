_gg_repos_cache="$DOTFILES_ZSH_CACHE/gg-repos"

_gg_refresh() {
  has_cmd gh || return 0
  local tmp="$_gg_repos_cache.$$"
  mkdir -p "${_gg_repos_cache:h}"
  if gh repo list fredrir --limit 1000 --json name -q '.[].name' >"$tmp" 2>/dev/null && [[ -s $tmp ]]; then
    mv -f "$tmp" "$_gg_repos_cache"
  else
    rm -f "$tmp"
  fi
}

_gg_stale=(${~_gg_repos_cache}(N.mh+24))
if [[ ! -f $_gg_repos_cache ]] || ((${#_gg_stale})); then
  defer _gg_refresh
fi
unset _gg_stale

_gg() {
  local -a repos
  [[ -r $_gg_repos_cache ]] && repos=("${(@f)$(<$_gg_repos_cache)}")
  ((${#repos})) && compadd -- "${repos[@]}"
}
compdef _gg gg

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
    alias gs='_git_from_root status -u'
    alias gc='_git_from_root commit -m'
    alias gca='git add -A && git commit --amend --no-edit && git push --force-with-lease --force-if-includes'
    alias gcm='_git_from_root commit -m'
    alias gp='_git_from_root pull --autostash --rebase'
    alias gpp="_git_from_root push"
    alias gl='_git_from_root log'
    alias gd='_git_from_root diff'
    alias gff='gppf .'
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

ga() {
  local exclude_targets=0
  local -a targets=()

  while (($#)); do
    case "$1" in
    -x)
      exclude_targets=1
      ;;
    --)
      shift
      targets+=("$@")
      break
      ;;
    *)
      targets+=("$1")
      ;;
    esac
    shift
  done

  if ((exclude_targets)); then
    local -a excludes=()
    local target

    for target in "${targets[@]}"; do
      excludes+=(":(exclude)$target")
    done

    _git_from_root add -- . "${excludes[@]}"
  else
    if (($#targets)); then
      _git_from_root add -- "${targets[@]}"
    else
      _git_from_root add -- .
    fi
  fi
}

_ga_changed_files() {
  _git_root || return 1
  local root=$REPLY
  local output
  local -a files

  output=$(command git -C "$root" ls-files \
    --modified \
    --deleted \
    --others \
    --exclude-standard \
    --deduplicate \
    -z 2>/dev/null)

  files=("${(@0)output}")
  ((${#files})) || return 1

  compadd -- "${files[@]}"
}

_ga() {
  _arguments \
    '-x[exclude target(s) from git add]' \
    '*:target:_ga_changed_files'
}

compdef _ga ga
