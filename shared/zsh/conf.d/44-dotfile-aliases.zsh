# Workspace setup
unalias dps 2>/dev/null # Was docker ps

dppf() {
  local source=${HOST}
  local execution="-t \"cd ~/dotfiles && git pull --autostash\""

  if [[source == "macie"]]; then
  "/$HOME/.local/bin/gppf" && "ssh $((execution))"
  fi
}


# Git
alias gdd="git-discard"

# Dotfile
alias dot="dotfile"
alias dots="dotfile sync"
alias dpp="dotfile sync -p"

alias pp="hwire -i"