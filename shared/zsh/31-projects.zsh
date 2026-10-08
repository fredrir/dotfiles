alias cdi="cd $HOME/infra"

# Projects
export PROJECTS="$HOME/projects"
alias cdpw="cd $PROJECTS/wez-plugins/vertical-tabs"
alias cdwp="cd $PROJECTS/wez-plugins/vertical-tabs"
alias cdpe="cd $PROJECTS/elvfast"
alias cdps="cd $PROJECTS/shucked"
alias cdpc="cd $PROJECTS/claude-usage-macos"

export APPKOM="$HOME/appkom"
alias cda="cd $APPKOM"

# llunde
export LLUNDE="$HOME/llunde"
export PYPARSER="$LLUNDE/pyparser"
alias cdl="cd  $LLUNDE"
alias cdlp="cd $PYPARSER"
alias cdlf="cd $LLUNDE/frontend"
alias cdlb="cd $LLUNDE/backend"

alias rr='direnv exec "$PYPARSER" "$LLUNDE/pyparser/.venv/bin/pyparser-review"'

# wez-vtabs
alias ww="just --justfile $HOME/projects/wez-plugins/vertical-tabs/justfile"
