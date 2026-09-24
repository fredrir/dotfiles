alias cdd="cd $DOTFILES"
alias cdn="cd $DOTFILES/shared/nvim"
alias cdw="cd $DOTFILES/shared/wezterm"
alias cdz="cd $DOTFILES/shared/zsh"

# Git
alias gdd="git-discard"

# Dotfile
alias dot="dotfile"
alias dots="dotfile sync"
alias dpp="dotfile sync -p"

alias pp="hwire -i"

alias ss="sysinfo -p"

if [[ -n "$LINUX" ]]; then
  alias sss="/usr/bin/ss"
fi

if [[ $PWD == "$DOTFILES" ]]; then
  export SOPS_AGE_KEY_FILE="$DOTFILES/config/age/keys.txt"
fi
