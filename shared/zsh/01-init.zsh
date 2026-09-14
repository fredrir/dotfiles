# ~/.zshrc
if [[ -n "$AGENT_SHELL" ]]; then
  source "$ZCONF/02-utils.zsh"
  source "$ZCONF/03-paths.zsh"
  return 0
fi

for _zsh_module in "$ZCONF"/{0[2-9],[1-9][0-9]}-*.zsh(N); do
  source "$_zsh_module"
done
unset _zsh_module