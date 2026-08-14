if command -v pnpm >/dev/null; then
  _pnpm_comp_cache="$HOME/.cache/zsh/pnpm-completion.zsh"
  if [[ ! -f "$_pnpm_comp_cache" || "$commands[pnpm]" -nt "$_pnpm_comp_cache" ]]; then
    mkdir -p "${_pnpm_comp_cache:h}"
    pnpm completion zsh > "$_pnpm_comp_cache" 2>/dev/null
  fi
  source "$_pnpm_comp_cache"
  unset _pnpm_comp_cache
fi

# oh-my-zsh already runs compinit; running it a second time re-dumps the
# completion cache on every shell start.
if ! (( $+functions[compdef] )); then
  autoload -Uz compinit
  compinit
fi

# Completions for the repo's Rust tools, regenerated whenever the binary is
# newer than the cache (same pattern as pnpm above, but after compinit since
# the generated script registers itself with compdef). The explicit paths
# avoid the system's Mach-O `size` and anything else of the same name.
for _tool in count gpp path size; do
  _tool_bin="$HOME/.local/bin/$_tool"
  [[ -x "$_tool_bin" ]] || continue
  _tool_comp_cache="$HOME/.cache/zsh/$_tool-completion.zsh"
  if [[ ! -f "$_tool_comp_cache" || "$_tool_bin" -nt "$_tool_comp_cache" ]]; then
    mkdir -p "${_tool_comp_cache:h}"
    "$_tool_bin" --completions zsh > "$_tool_comp_cache" 2>/dev/null \
      || : > "$_tool_comp_cache"
  fi
  source "$_tool_comp_cache"
done
unset _tool _tool_bin _tool_comp_cache

zstyle ':completion:*' matcher-list '' 'm:{a-zA-Z}={A-Za-z}'
