alias -g NV='| nvim -R -'
alias -g CP="| dclip"

alias f='find . -type f -name'
alias c="clear"
alias u="uname -mrs"

alias n="nvim"
alias nn="nvim ."
alias v="nvim"
alias vv="nvim ."

alias l="ls"
alias la="ls -a"
alias ll="ls -lah"
alias lld="eza -lahX --no-permissions --no-filesize --no-user --time=modified --sort=modified" # Last modified
alias llc="eza -lahX --no-permissions --no-filesize --no-user --time=created --sort=modified"

alias cdf="cd ../frontend"
alias cdb="cd ../backend"
alias cdj='cd "$OLDPWD"'
alias cdh="cd $HOME"
alias cdp="cd $HOME/projects"
alias cds="cd $HOME/.ssh/config.d"
alias cdo="cd $HOME/Documents/main/.obsidian"
alias cdc="cd $CONFIG"

alias ..='cd ..'
alias ...='cd ../..'
alias ....='cd ../../..'
alias cd..="cd .."
alias cd...="cd ../.."

alias mkdir="mkdir -p"

alias grep="grep --color=auto"
alias fgrep="fgrep --color=auto"

alias port="portview"

alias exz="exec zsh"

alias disk="ncdu"

alias cleanup="kondo"

alias untar="tar -xzf"

has_cmd bat && alias cat='bat -pp'

alias sshmux="ssh -O check"
alias sshmux-exit="ssh -O exit"
alias wez="wezterm cli --no-auto-start"

# IDE
alias charm="pycharm . &!"

# Tooling
alias docku="docker compose up --build"
alias dockd="docker compose down -v"
alias dockseed="docker compose exec backend pnpm db:seed"
alias dockus="docker compose down -v && docker compose up --build -d && docker compose exec backend pnpm db:seed && docker compose logs -f backend"

alias penv="python -m venv .venv && source .venv/bin/activate"

# Linux
if [[ -n $LINUX ]]; then
  alias pacS="sudo pacman -S --needed"
  alias pacnew='pacdiff -s'
  alias paclog="paclog | tail -50"
  alias yay="paru"
fi