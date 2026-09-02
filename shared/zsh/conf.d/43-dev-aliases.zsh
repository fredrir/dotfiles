# AI
alias opencode-stats="$HOME/.opencode/bin/opencode stats --days 7 --models 10 --tools 20"

# Projects
alias cdpw="cd $HOME/projects/wez-plugins/vertical-tabs"
alias cdwp="cd $HOME/projects/wez-plugins/vertical-tabs"

alias ww='just --justfile /Users/fredrir/projects/wez-plugins/vertical-tabs/justfile'
alias wwr='just --justfile /Users/fredrir/projects/wez-plugins/vertical-tabs/justfile restart'

alias cdpe="cd $HOME/projects/elvfast"

# Tooling
alias docku="docker compose up --build"
alias dockd="docker compose down -v"
alias dockseed="docker compose exec backend pnpm db:seed"
alias dockus="docker compose down -v && docker compose up --build -d && docker compose exec backend pnpm db:seed && docker compose logs -f backend"

alias dockexp="docker exec -e SAMPLES_DIR=/samples/exams llunde-pyparser-worker"

alias pyparser-restart="ssh leploy 'cd /opt/pyparser && docker compose restart'"

alias penv="python -m venv .venv && source .venv/bin/activate"