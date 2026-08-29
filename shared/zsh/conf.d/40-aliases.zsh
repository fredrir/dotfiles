alias -g NV='| nvim -R -'

alias mkdir="mkdir -p"

alias grep="grep --color=auto"
alias fgrep="fgrep --color=auto"
alias f='find . -type f -name'

alias port="portview"

alias disk="ncdu"

alias n="nvim"
alias nn="nvim ."
alias v="nvim"
alias vv="nvim ."

alias la="ls -a"
alias l="ls"
alias ll="ls -lah"
alias lld="eza -lahX --no-permissions --no-filesize --no-user --time=modified --sort=modified" # Last modified
alias llc="eza -lahX --no-permissions --no-filesize --no-user --time=created --sort=modified"

alias c="clear"

alias untar="tar -xzf"

(($+commands[bat])) && alias cat='bat -pp'

# Navigation
alias ..='cd ..'
alias ...='cd ../..'
alias ....='cd ../../..'
alias cd..="cd .."
alias cd...="cd ../.."

alias cdh="cd $HOME"
alias cdc="cd $CONFIG"
alias cdd="cd $HOME/dotfiles"
alias cdn="cd $NVIM"
alias cdz="cd $CONFIG/zsh/conf.d"
alias cds="cd $HOME/.ssh/config.d"

alias sshmux="ssh -O check"
alias sshmux-exit="ssh -O exit"
alias wez="wezterm cli"

alias cdp="cd $HOME/projects"
alias cdf="cd ../frontend"
alias cdb="cd ../backend"

alias cdo="cd $HOME/Documents/main/.obsidian"

alias cdj='cd "$OLDPWD"' # cd jump to last directory

# Other

alias u="uname -mrs"

alias cleanup="kondo" # Cleanup build output

# Git
alias gca='git add -A && git commit --amend --no-edit && git push --force-with-lease --force-if-includes'