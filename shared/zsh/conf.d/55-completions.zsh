if command -v pnpm >/dev/null; then
  _pnpm_comp_cache="$HOME/.cache/zsh/pnpm-completion.zsh"
  if [[ ! -f "$_pnpm_comp_cache" || "$commands[pnpm]" -nt "$_pnpm_comp_cache" ]]; then
    mkdir -p "${_pnpm_comp_cache:h}"
    pnpm completion zsh >"$_pnpm_comp_cache" 2>/dev/null
  fi
  source "$_pnpm_comp_cache"
  unset _pnpm_comp_cache
fi

if ! (($+functions[compdef])); then
  autoload -Uz compinit
  compinit
fi

for _tool in agent-hop count dotfile-format dotfmt flatten gdd gget gpp hpull hpush hwire mux-route path size; do
  if [[ "$_tool" == gdd ]]; then
    _tool_bin="$HOME/.local/bin/git-discard"
  else
    _tool_bin="$HOME/.local/bin/$_tool"
  fi
  [[ -x "$_tool_bin" ]] || continue
  _tool_comp_cache="$HOME/.cache/zsh/$_tool-completion.zsh"
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

_tools_comp_cache="$HOME/.cache/zsh/tools-completion.zsh"
[[ -r "$_tools_comp_cache" ]] && source "$_tools_comp_cache"
unset _tools_comp_cache

zstyle ':completion:*' matcher-list '' 'm:{a-zA-Z}={A-Za-z}'
