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
for _tool in count flatten gdd gpp hwire path size; do
  # GNU dd by fzf-tab on macOS.
  if [[ "$_tool" == gdd ]]; then
    _tool_bin="$HOME/.local/bin/git-discard"
  else
    _tool_bin="$HOME/.local/bin/$_tool"
  fi
  [[ -x "$_tool_bin" ]] || continue
  _tool_comp_cache="$HOME/.cache/zsh/$_tool-completion.zsh"
  if [[ ! -f "$_tool_comp_cache" || "$_tool_bin" -nt "$_tool_comp_cache" ]]; then
    mkdir -p "${_tool_comp_cache:h}"
    "$_tool_bin" --completions zsh > "$_tool_comp_cache" 2>/dev/null \
      || : > "$_tool_comp_cache"
  fi
  source "$_tool_comp_cache"
  [[ "$_tool" == gdd ]] && compdef _gdd git-discard
done
unset _tool _tool_bin _tool_comp_cache

# dmux uses clap's dynamic completer instead of the static --completions
# flag: COMPLETE=zsh emits a runtime shim that asks the binary at completion
# time, which is how session names stay live. Same cache pattern as above,
# except the binary resolves through PATH (matching the ssa/ssm wrappers,
# so a `cargo install`ed dmux gets completions too), and — because the shim
# embeds the binary's absolute path — the cache records that path on its
# first line and regenerates when it changes, not just when the binary is
# newer.
_dmux_bin="${commands[dmux]:-$HOME/.local/bin/dmux}"
if [[ -x "$_dmux_bin" ]]; then
  _dmux_comp_cache="$HOME/.cache/zsh/dmux-completion.zsh"
  _dmux_comp_src=""
  [[ -f "$_dmux_comp_cache" ]] && IFS= read -r _dmux_comp_src < "$_dmux_comp_cache"
  if [[ "$_dmux_comp_src" != "# $_dmux_bin" || "$_dmux_bin" -nt "$_dmux_comp_cache" ]]; then
    mkdir -p "${_dmux_comp_cache:h}"
    { print -r -- "# $_dmux_bin" &&
      COMPLETE=zsh "$_dmux_bin" 2>/dev/null } > "$_dmux_comp_cache" \
      || print -r -- "# $_dmux_bin" > "$_dmux_comp_cache"
  fi
  source "$_dmux_comp_cache"
fi
unset _dmux_bin _dmux_comp_cache _dmux_comp_src

zstyle ':completion:*' matcher-list '' 'm:{a-zA-Z}={A-Za-z}'
