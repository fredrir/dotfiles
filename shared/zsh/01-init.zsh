# ~/.zshrc
# zsh-build: omit
if [[ -z $ZSH_BUILD_SKIP && -r $DOTFILES_ZSH_CACHE/build/zshrc.zsh ]]; then
  source "$DOTFILES_ZSH_CACHE/build/zshrc.zsh"
  return
fi

if [[ -n "$AGENT_SHELL" ]]; then
  source "$ZCONF/02-utils.zsh"
  source "$ZCONF/03-paths.zsh"
  return 0
fi

for _zsh_module in "$ZCONF"/{0[2-9],[1-9][0-9]}-*.zsh(N); do
  source "$_zsh_module"
done
unset _zsh_module

# opencode
export PATH=/Users/fredrir/.opencode/bin:$PATH
