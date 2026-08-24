# zmodload zsh/zprof

ZCONF="$HOME/.config/zsh/conf.d"

if [[ -n ${AGENT_SHELL:-} || -n ${CLAUDECODE:-} || -n ${AI_AGENT:-} ]]; then

  for file in "$ZCONF"/[12][0-9]-*.zsh(N); do
    [[ -f "$file" ]] || continue
    [[ "$file" == "$ZCONF/10-env.zsh" ]] && continue
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

# zprof
