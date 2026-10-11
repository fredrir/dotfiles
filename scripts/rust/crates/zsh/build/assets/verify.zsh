# Sourced after ~/.zshrc by `zsh-build verify`, aliases off: dumps the state as NUL-separated
# table, name, value records once the deferred queue is empty, then kills the shell.
typeset -g __zbv_histfile=$HISTFILE __zbv_scratch_histfile=@HISTFILE@
HISTFILE=$__zbv_scratch_histfile

__zbv_record() {
  builtin print -rn -- "$1"$'\0'"${2//$'\0'/\\0}"$'\0'"${3//$'\0'/\\0}"$'\0'
}

__zbv_dump() {
  builtin emulate -L zsh -o extended_glob
  # Prefixed, so no local hides a global
  local __zbv_name __zbv_value __zbv_line __zbv_keymap __zbv_flag __zbv_dir __zbv_file
  local -a __zbv_words
  local -A __zbv_hash
  # Values of this call or this session, not of what startup set
  local -a __zbv_skip=('*' @ '#' '?' '$' ! - _ 0 argv ARGC status pipestatus LINENO PPID
    RANDOM SECONDS HISTCMD TTY TTYIDLE COLUMNS LINES ZSH_EVAL_CONTEXT zsh_eval_context
    ZSH_SUBSHELL OPTIND OPTARG TRY_BLOCK_ERROR TRY_BLOCK_INTERRUPT)
  # Completion scripts the bundle leaves to compinit, loaded as `autoload -U` loads them
  for __zbv_dir in @COMPLETIONS@; do
    (( ${fpath[(Ie)$__zbv_dir]} )) || continue
    for __zbv_file in "$__zbv_dir"/*(N.); do
      () { builtin emulate -L zsh -o no_aliases; builtin source $__zbv_file }
    done
  done
  {
    for __zbv_name __zbv_value in "${(@kv)__zbv_options}"; do
      __zbv_record options $__zbv_name $__zbv_value
    done
    for __zbv_name __zbv_value in "${(@kv)aliases}"; do
      __zbv_record aliases $__zbv_name $__zbv_value
    done
    for __zbv_name __zbv_value in "${(@kv)galiases}"; do
      __zbv_record global-aliases $__zbv_name $__zbv_value
    done
    for __zbv_name __zbv_value in "${(@kv)saliases}"; do
      __zbv_record suffix-aliases $__zbv_name $__zbv_value
    done
    for __zbv_name __zbv_value in "${(@kv)functions}"; do
      __zbv_record functions $__zbv_name $__zbv_value
    done
    for __zbv_name __zbv_value in "${(@kv)widgets}"; do
      __zbv_record widgets $__zbv_name $__zbv_value
    done
    for __zbv_name __zbv_value in "${(@kv)nameddirs}"; do
      __zbv_record named-dirs $__zbv_name $__zbv_value
    done
    for __zbv_name __zbv_value in "${(@kv)modules}"; do
      __zbv_record modules $__zbv_name $__zbv_value
    done
    for __zbv_name in ${(k)dis_builtins} ${(k)dis_reswords} ${(k)dis_aliases} \
      ${(k)dis_galiases} ${(k)dis_saliases} ${(k)dis_functions} $dis_patchars; do
      __zbv_record disabled $__zbv_name disabled
    done
    for __zbv_line in ${(f)"$(builtin functions -M)"}; do
      __zbv_record math-functions ${${(z)__zbv_line}[3]} $__zbv_line
    done
    for __zbv_line in ${(f)"$(builtin trap)"}; do
      __zbv_record traps ${${(z)__zbv_line}[-1]} $__zbv_line
    done
    for __zbv_line in ${(f)"$(builtin bindkey -lL)"}; do
      __zbv_record keymaps $__zbv_line $__zbv_line
    done
    for __zbv_keymap in $keymaps; do
      for __zbv_line in ${(f)"$(builtin bindkey -LM $__zbv_keymap)"}; do
        __zbv_words=(${(z)__zbv_line})
        __zbv_record bindings "$__zbv_keymap ${__zbv_words[1,-2]}" $__zbv_words[-1]
      done
    done
    for __zbv_line in ${(f)"$(builtin zstyle -L)"}; do
      __zbv_words=(${(z)__zbv_line})
      __zbv_flag=
      [[ $__zbv_words[2] == -e ]] && __zbv_flag='-e ' && shift __zbv_words
      __zbv_record zstyles "$__zbv_flag${(Q)__zbv_words[2]} ${(Q)__zbv_words[3]}" \
        "${(F)${(@Q)__zbv_words[4,-1]}}"
    done
    for __zbv_name in ${(k)parameters}; do
      [[ ${parameters[$__zbv_name]} == *(local|hideval)* ]] && continue
      (( ${__zbv_skip[(Ie)$__zbv_name]} )) && continue
      __zbv_record types $__zbv_name ${parameters[$__zbv_name]}
      case ${parameters[$__zbv_name]} in
        association*)
          __zbv_hash=("${(@Pkv)__zbv_name}")
          __zbv_value=
          for __zbv_line in ${(ko)__zbv_hash}; do
            __zbv_value+="$__zbv_line=$__zbv_hash[$__zbv_line]"$'\n'
          done
          ;;
        array*) __zbv_value=${(PF)__zbv_name} ;;
        *) __zbv_value=${(P)__zbv_name} ;;
      esac
      [[ $__zbv_name == HISTFILE && $HISTFILE == $__zbv_scratch_histfile ]] &&
        __zbv_value=$__zbv_histfile
      __zbv_record parameters $__zbv_name $__zbv_value
    done
    __zbv_record end '' ''
  } >| @DUMP@
}

__zbv_ready() {
  # Read before emulate, which changes them for the call
  local -A __zbv_options=("${(@kv)options}")
  builtin emulate -L zsh
  (( $#_defer_queue )) && return
  builtin zle -F $1
  __zbv_dump
  builtin kill -KILL $$
}

typeset -g __zbv_fd
builtin exec {__zbv_fd}</dev/null
builtin zle -F $__zbv_fd __zbv_ready
