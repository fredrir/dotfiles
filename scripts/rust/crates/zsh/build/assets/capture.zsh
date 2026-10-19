# Sourced after ~/.zshrc by zsh-build, aliases off: wraps zsh-autosuggestions' first binding,
# dumps what it changed as NUL-separated table, name, value records, then kills the shell.
HISTFILE=@HISTFILE@

__zbc_record() {
  builtin print -rn -- "$1"$'\0'"${2//$'\0'/\\0}"$'\0'"${3//$'\0'/\\0}"$'\0'
}

# Global parameters by name, as type and value, into the association named $1
__zbc_snapshot() {
  local __zbc_name __zbc_type __zbc_value
  local -A __zbc_table
  for __zbc_name in ${(k)parameters}; do
    __zbc_type=${parameters[$__zbc_name]}
    [[ $__zbc_type == *(local|special|hideval)* ]] && continue
    if [[ $__zbc_type == association* ]]; then
      __zbc_table=("${(@Pkv)__zbc_name}")
      __zbc_value=${(j:\0:)${(@okv)__zbc_table}}
    else
      __zbc_value=${(Pj:\0:)__zbc_name}
    fi
    builtin eval "$1[\$__zbc_name]=\$__zbc_type=\$__zbc_value"
  done
}

__zbc_capture() {
  local REPLY __zbc_name
  _zsh_build_autosuggest_key
  local __zbc_key=$REPLY
  local -A __zbc_functions=("${(@kv)functions}") __zbc_widgets=("${(@kv)widgets}")
  local -A __zbc_before __zbc_after
  # Reading a parameter can load the module behind it; the second pass sees it loaded
  __zbc_snapshot __zbc_before
  __zbc_before=()
  __zbc_snapshot __zbc_before
  __zbc_original "$@"
  __zbc_snapshot __zbc_after
  {
    __zbc_record key '' $__zbc_key
    for __zbc_name in ${(ok)functions}; do
      (( ${+__zbc_functions[$__zbc_name]} )) &&
        [[ $__zbc_functions[$__zbc_name] == "$functions[$__zbc_name]" ]] && continue
      __zbc_record functions $__zbc_name $functions[$__zbc_name]
    done
    for __zbc_name in ${(ok)__zbc_functions}; do
      (( $+functions[$__zbc_name] )) || __zbc_record removed function $__zbc_name
    done
    for __zbc_name in ${(ok)widgets}; do
      (( ${+__zbc_widgets[$__zbc_name]} )) &&
        [[ $__zbc_widgets[$__zbc_name] == "$widgets[$__zbc_name]" ]] && continue
      __zbc_record widgets $__zbc_name $widgets[$__zbc_name]
    done
    for __zbc_name in ${(ok)__zbc_widgets}; do
      (( $+widgets[$__zbc_name] )) || __zbc_record removed widget $__zbc_name
    done
    for __zbc_name in ${(ok)__zbc_after}; do
      (( ${+__zbc_before[$__zbc_name]} )) &&
        [[ $__zbc_before[$__zbc_name] == "$__zbc_after[$__zbc_name]" ]] && continue
      __zbc_record parameters $__zbc_name $parameters[$__zbc_name]
    done
    for __zbc_name in ${(ok)__zbc_before}; do
      (( ${+__zbc_after[$__zbc_name]} )) || __zbc_record removed parameter $__zbc_name
    done
    for __zbc_name in ${(ok)_ZSH_AUTOSUGGEST_BIND_COUNTS}; do
      __zbc_record counts $__zbc_name $_ZSH_AUTOSUGGEST_BIND_COUNTS[$__zbc_name]
    done
    __zbc_record end '' ''
  } >| @DUMP@
  builtin kill -KILL $$
}

__zbc_install() {
  (( $+functions[_zsh_autosuggest_bind_widgets] )) || return
  builtin functions -c _zsh_autosuggest_bind_widgets __zbc_original
  _zsh_autosuggest_bind_widgets() { __zbc_capture "$@" }
}

if (( $+functions[_zsh_autosuggest_bind_widgets] )); then
  __zbc_install
elif (( $+functions[defer] )); then
  defer __zbc_install
fi

# Once the queue is empty without the binding having run
__zbc_ready() {
  (( $#_defer_queue )) && return
  builtin zle -F $1
  { __zbc_record unbound '' ''; __zbc_record end '' '' } >| @DUMP@
  builtin kill -KILL $$
}

typeset -g __zbc_fd
builtin exec {__zbc_fd}</dev/null
builtin zle -F $__zbc_fd __zbc_ready
