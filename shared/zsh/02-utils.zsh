typeset -gU path PATH
typeset -gU plugins
typeset -gaU zsh_plugin_path
typeset -gaU zsh_plugin_sources
typeset -U fpath

add_path() {
  local -a valid_paths=()
  local dir
  for dir in "$@"; do
    [[ -d "$dir" ]] && valid_paths+=("$dir")
  done
  path=("${valid_paths[@]}" "${path[@]}")
}

add_plugin_path() {
  local dir
  for dir in "$@"; do
    [[ -d $dir ]] && zsh_plugin_path+=($dir)
  done
}

add_plugins() {
  local spec name dir file
  for spec in "$@"; do
    name=${spec##*:}
    ((${+commands[${spec%%:*}]})) || continue

    for dir in $zsh_plugin_path; do
      for file in $dir/$name/$name.zsh $dir/$name.zsh; do
        [[ -r $file ]] || continue
        zsh_plugin_sources+=($file)
        continue 3
      done
    done

    [[ -d $ZSH/plugins/$name ]] && plugins+=($name)
  done
}

has_cmd() { (($+commands[$1])); }
dir_exists() { [[ -d "$1" ]]; }

add_fpath() {
  local dir
  for dir in "$@"; do
    [[ -d "$dir" ]] && fpath=("$dir" $fpath)
  done
}

cached_eval() {
  local cache="$DOTFILES_ZSH_CACHE/$1.zsh" bin
  shift
  bin=${commands[$1]:-$1}
  [[ -x $bin ]] || return 0

  if [[ ! -f $cache || $bin -nt $cache ]]; then
    mkdir -p "${cache:h}"
    "$@" >"$cache" 2>/dev/null || return 0
  fi

  source "$cache"
}

typeset -ga _defer_queue

defer() {
  (($#_defer_queue)) || _defer_schedule
  _defer_queue+=("$*")
}

_defer_schedule() {
  local fd
  exec {fd}</dev/null
  zle -F $fd _defer_flush
}

_defer_flush() {
  local fd=$1
  zle -F $fd
  exec {fd}<&-

  while (($#_defer_queue)); do
    eval $_defer_queue[1]
    shift _defer_queue
    ((KEYS_QUEUED_COUNT || PENDING)) && {
      _defer_schedule
      return
    }
  done

  (($+functions[_zsh_autosuggest_start])) && _zsh_autosuggest_start
  zle reset-prompt
}
zle -N _defer_flush

[[ $OSTYPE == linux* ]] && LINUX=1
[[ -e /etc/arch-release ]] && ARCHLINUX=1
[[ $OSTYPE == darwin* ]] && MACOS=1
