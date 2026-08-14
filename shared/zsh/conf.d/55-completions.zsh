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
# the generated script registers itself with compdef). The explicit path
# avoids the system's Mach-O `size`.
if [[ -x "$HOME/.local/bin/size" ]]; then
  _size_comp_cache="$HOME/.cache/zsh/size-completion.zsh"
  if [[ ! -f "$_size_comp_cache" || "$HOME/.local/bin/size" -nt "$_size_comp_cache" ]]; then
    mkdir -p "${_size_comp_cache:h}"
    "$HOME/.local/bin/size" --completions zsh > "$_size_comp_cache" 2>/dev/null \
      || : > "$_size_comp_cache"
  fi
  source "$_size_comp_cache"
  unset _size_comp_cache
fi

zstyle ':completion:*' matcher-list '' 'm:{a-zA-Z}={A-Za-z}'
