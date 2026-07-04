export PAGER=less
export LESS="-R -F --mouse --wheel-lines=3"

export CONFIG="$HOME/.config"
export NVIM="$CONFIG/nvim"

command -v direnv >/dev/null 2>&1 && eval "$(direnv hook zsh)"
