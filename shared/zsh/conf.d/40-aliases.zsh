alias cp="cp -i"
alias mv="mv -i"
alias rm="rm -i"
alias mkdir="mkdir -p"

alias grep="grep --color=auto"
alias fgrep="fgrep --color=auto"
alias f='find . -type f -name'

alias disk="ncdu"

alias n="nvim"
alias nn="nvim ."
alias v="nvim"
alias vv="nvim ."

alias la="ls -a"
alias ll="ls -l"

alias untar="tar -xzf"

(( $+commands[bat] )) && alias cat='bat -pp'


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
alias cdp="cd ~/llunde/pyparser && python -m venv .venv && source .venv/bin/activate"
alias cdz="cd $CONFIG/zsh/conf.d"
alias cds="cd $HOME/.ssh/config.d"

alias sshmux="ssh -O check"    
alias sshmux-exit="ssh -O exit" 

alias cdf="cd ../frontend"
alias cdb="cd ../backend"

alias cdj='cd "$OLDPWD"' # cd jump to last directory

alias gca='git add -A && git commit --amend --no-edit && git push --force-with-lease'
alias gdd='git-discard'

alias docku="docker compose up --build"
alias dockd="docker compose down -v"
alias dockseed="docker compose exec backend pnpm db:seed"
alias dockus="docker compose down -v && docker compose up --build -d && docker compose exec backend pnpm db:seed && docker compose logs -f backend"

alias dockexp="docker exec -e SAMPLES_DIR=/samples/exams llunde-pyparser-worker"

alias pyparser-restart="ssh leploy 'cd /opt/pyparser && docker compose restart'"

alias penv="python -m venv .venv && source .venv/bin/activate"

alias claude="claude --dangerously-skip-permissions"
alias codex="codex --yolo"
