for _bin in "$DOTFILES_COMPILED"/*(N); do
  cached_eval "${_bin:t}-completion" "$_bin" --completions zsh
done
unset _bin

cached_eval wezterm-completion wezterm shell-completion --shell zsh

compdef _gdd git-discard
compdef _mux-route attach_mux s

if (( $+functions[_wezterm] )); then
  _wezterm_cli() {
    words=(wezterm cli "${(@)words[2,-1]}")
    (( CURRENT++ ))
    _wezterm
  }
  compdef _wezterm_cli wez mux
fi

_tools_comp_cache="$DOTFILES_ZSH_CACHE/tools-completion.zsh"
[[ -r "$_tools_comp_cache" ]] && source "$_tools_comp_cache"
unset _tools_comp_cache

zstyle ':completion:*' matcher-list '' 'm:{a-zA-Z}={A-Za-z}'
