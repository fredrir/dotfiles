for _tool in agent-hop count dcloud dotfile-format dotfmt flatten gdd gget gppf hpull hpush hwire hwtune mux-route path size sysinfo tmux-workspace; do
  if [[ "$_tool" == gdd ]]; then
    _tool_bin="$DOTFILES_COMPILED/git-discard"
  else
    _tool_bin="$DOTFILES_COMPILED/$_tool"
  fi
  [[ -x "$_tool_bin" ]] || continue
  _tool_comp_cache="$DOTFILES_ZSH_CACHE/$_tool-completion.zsh"
  if [[ ! -f "$_tool_comp_cache" || "$_tool_bin" -nt "$_tool_comp_cache" ]]; then
    mkdir -p "${_tool_comp_cache:h}"
    "$_tool_bin" --completions zsh >"$_tool_comp_cache" 2>/dev/null ||
      : >"$_tool_comp_cache"
  fi
  source "$_tool_comp_cache"
  [[ "$_tool" == gdd ]] && compdef _gdd git-discard
  [[ "$_tool" == mux-route ]] && compdef _mux-route mux
done
unset _tool _tool_bin _tool_comp_cache

_tools_comp_cache="$DOTFILES_ZSH_CACHE/tools-completion.zsh"
[[ -r "$_tools_comp_cache" ]] && source "$_tools_comp_cache"
unset _tools_comp_cache

zstyle ':completion:*' matcher-list '' 'm:{a-zA-Z}={A-Za-z}'
