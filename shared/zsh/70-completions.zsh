for _bin in "$DOTFILES_COMPILED"/*(N); do
  cached_eval "${_bin:t}-completion" "$_bin" --completions zsh
done
unset _bin

compdef _gdd git-discard
compdef _mux-route mux

_tools_comp_cache="$DOTFILES_ZSH_CACHE/tools-completion.zsh"
[[ -r "$_tools_comp_cache" ]] && source "$_tools_comp_cache"
unset _tools_comp_cache

zstyle ':completion:*' matcher-list '' 'm:{a-zA-Z}={A-Za-z}'
