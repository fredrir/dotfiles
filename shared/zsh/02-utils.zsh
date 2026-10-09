# shuck: disable-file=C001
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
_b64() {
  emulate -L zsh -o no_multibyte
  local table=ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/
  local in=$1 out='' c1 c2 c3
  local -i i n=${#in} chunk
  for ((i = 1; i <= n; i += 3)); do
    c1=${in[i]} c2=${in[i+1]} c3=${in[i+2]}
    ((chunk = #c1 << 16 | (${#c2} ? #c2 : 0) << 8 | (${#c3} ? #c3 : 0)))
    out+="${table[(chunk >> 18 & 63) + 1]}${table[(chunk >> 12 & 63) + 1]}"
    out+="${${c2:+${table[(chunk >> 6 & 63)+1]}}:-=}${${c3:+${table[(chunk & 63)+1]}}:-=}"
  done
  REPLY=$out
}
set_user_var() {
  local REPLY
  _b64 "$2"
  printf '\e]1337;SetUserVar=%s=%s\a' "$1" "$REPLY"
}

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
  # Queued by deferred work: runs right after it
  if ((${+_defer_at})); then
    _defer_queue[_defer_at,_defer_at-1]=("$*")
    ((_defer_at++))
  else
    _defer_queue+=("$*")
  fi
}

_defer_schedule() {
  local fd
  exec {fd}</dev/null
  # As a widget, so it sees the keys typed while it runs
  zle -F -w $fd _defer_flush
  autoload -Uz add-zle-hook-widget
  add-zle-hook-widget line-init _defer_typeahead
  add-zle-hook-widget line-finish _defer_drain
}

_defer_unhook() {
  add-zle-hook-widget -d line-init _defer_typeahead
  add-zle-hook-widget -d line-finish _defer_drain
}

# zle reads keys typed before the prompt ahead of the queue; run it first
_defer_typeahead() {
  ((KEYS_QUEUED_COUNT || PENDING)) && _defer_drain
  return 0
}

# A line accepted before the queue empties runs after the rest of it
_defer_drain() {
  while (($#_defer_queue)); do _defer_next; done
  _defer_unhook
}

_defer_next() {
  local -i _defer_at=2
  # Under job control, each pipeline makes zle redraw the prompt
  if [[ -o monitor ]]; then
    unsetopt monitor
    { eval $_defer_queue[1]; } always { setopt monitor; }
  else
    eval $_defer_queue[1]
  fi
  shift _defer_queue
}

_defer_flush() {
  local fd=$1
  zle -F $fd
  exec {fd}<&-

  while (($#_defer_queue)); do
    _defer_next
    ((KEYS_QUEUED_COUNT || PENDING)) && {
      _defer_schedule
      return
    }
  done

  _defer_unhook
  (($+functions[_zsh_autosuggest_start])) && _zsh_autosuggest_start
  zle reset-prompt
}
zle -N _defer_flush

[[ $OSTYPE == linux* ]] && LINUX=1
[[ -e /etc/arch-release ]] && ARCHLINUX=1
[[ $OSTYPE == darwin* ]] && MACOS=1
