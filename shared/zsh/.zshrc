ZCONF="$HOME/.config/zsh/conf.d"

# An agent shell -- one started by a wrapper in 42-agents.zsh, or by anything
# those agents spawn in turn -- gets the machine layer only: environment and
# PATH. Aliases, functions, hooks, plugins and the prompt exist for a human at
# a keyboard; an agent reads them as the machine behaving strangely. The cost is
# not only correctness. Claude snapshots this shell once per session and
# re-sources the result before every command it runs, so a full load is paid
# for on every command.
#
# AGENT_SHELL is ours. The vendor variables cover an agent started outside a
# wrapper, and one nested inside another agent, where the wrappers are gone.
if [[ -n ${AGENT_SHELL:-} || -n ${CLAUDECODE:-} || -n ${AI_AGENT:-} ]]; then
  # 1x environment, 2x paths, nothing else: baseline zsh with a working
  # toolchain. direnv is left out deliberately -- it works through precmd and
  # chpwd hooks, and an agent shell is a one-shot `zsh -c` that never reaches a
  # prompt, so loading it would fork `direnv hook zsh` to register two hooks
  # that never fire. A project whose .envrc an agent genuinely needs says so
  # itself, in its own CLAUDE.md: `uv run ...`, or `direnv exec .` up front.
  for file in "$ZCONF"/[12][0-9]-*.zsh(N); do
    [[ -f "$file" ]] || continue
    source "$file"
  done

  [[ -f "$HOME/.local/bin/env" ]] && source "$HOME/.local/bin/env"
  return 0
fi

[[ -f "$ZCONF/05-ohmyzsh.zsh" ]] && source "$ZCONF/05-ohmyzsh.zsh"
[[ -f "$ZSH/oh-my-zsh.sh" ]] && source "$ZSH/oh-my-zsh.sh"

for file in "$ZCONF"/*.zsh; do
  [[ -f "$file" ]] || continue
  [[ "$file" == "$ZCONF/05-ohmyzsh.zsh" ]] && continue  # already sourced above
  source "$file"
done

[[ -f "$HOME/.local/bin/env" ]] && source "$HOME/.local/bin/env"
