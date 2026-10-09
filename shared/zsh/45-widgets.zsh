if has_cmd nvim; then
  _search_files() {
    zle -I
    command nvim +TerminalSearch
    zle reset-prompt
  }
  _search_grep() {
    zle -I
    command nvim -c \
      'autocmd VimEnter * ++once Telescope live_grep'
    zle reset-prompt
  }

  zle -N search_files _search_files
  zle -N search_grep _search_grep

elif has_cmd fzf; then
  zle -A fzf-file-widget search_files
fi

if has_cmd atuin; then
  export ATUIN_NOBIND=true
  cached_eval atuin-init-no-bind atuin init zsh --disable-ctrl-r --disable-up-arrow
  (($+functions[_zsh_autosuggest_strategy_atuin])) && export ZSH_AUTOSUGGEST_STRATEGY=(history atuin)

  if [[ ${__atuin_pty_proxy_owns_tty:-0} != 1 && $functions[_atuin_preexec] == *'atuin history start --hook'* ]]; then
    _atuin_preexec() {
      exec {_atuin_start_fd}< <(ATUIN_SHELL=zsh atuin history start --hook -- "$1" 2>/dev/null)
      __atuin_preexec_time=${EPOCHREALTIME-}
    }
    _atuin_precmd() {
      local EXIT=$? __atuin_precmd_time=${EPOCHREALTIME-} duration=''
      ((${+_atuin_start_fd})) || return
      if [[ -n $__atuin_preexec_time && -n $__atuin_precmd_time ]]; then
        printf -v duration %.0f $(((__atuin_precmd_time - __atuin_preexec_time) * 1000000000))
        ((duration < 0)) && duration=0
      fi
      (
        local id
        IFS= read -r id <&$_atuin_start_fd && [[ -n $id ]] &&
          atuin history end --hook --exit $EXIT ${duration:+--duration=$duration} -- $id
      ) >/dev/null 2>&1 &!
      exec {_atuin_start_fd}<&-
      unset _atuin_start_fd
    }
  fi

  zle -A atuin-search search_history
fi
