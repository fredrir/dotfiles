for _bin in "$DOTFILES_COMPILED"/*(N); do
  cached_eval "${_bin:t}-completion" "$_bin" --completions zsh
done
unset _bin

cached_eval wezterm-completion wezterm shell-completion --shell zsh
cached_eval llunde-completion llunde completion zsh

compdef _gdd git-discard
compdef _mux-route attach_mux s

if (($+functions[_wezterm])); then
  _wezterm_cli() {
    words=(wezterm cli "${(@)words[2,-1]}")
    ((CURRENT++))
    _wezterm
  }
  compdef _wezterm_cli wez mux
fi

_just_vtabs() {
  local -a args=("${(@)words[2,-1]}")
  local index=${args[(i)--justfile]} root=$PWD removed=0
  if ((index < $#args)); then
    root=${~args[index+1]:h}
    args[index,index+1]=()
    removed=2
  else
    while [[ $root != / && ! -f $root/justfile ]]; do root=${root:h}; done
  fi
  local tool=$root/target/xtask/wez-vtabs
  if ((CURRENT - removed > 2)) && [[ -x $tool && ${args[1]} == (deploy|build|dev|test|lint|status|setup) ]]; then
    (($+functions[_clap_dynamic_completer_wez_vtabs])) || eval "$(WEZ_VTABS_COMPLETE=zsh $tool)"
    words=(wez-vtabs "${(@)args}")
    ((CURRENT -= removed))
    _clap_dynamic_completer_wez_vtabs
  else
    _just "$@"
  fi
}
compdef _just_vtabs just

_tools_comp_cache="$DOTFILES_ZSH_CACHE/tools-completion.zsh"
[[ -r "$_tools_comp_cache" ]] && source "$_tools_comp_cache"
unset _tools_comp_cache

zstyle ':completion:*' matcher-list '' 'm:{a-zA-Z}={A-Za-z}'
