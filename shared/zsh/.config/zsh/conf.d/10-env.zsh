# Cross-platform environment (loaded on every machine).
export PAGER=less
export LESS="-R -F --mouse --wheel-lines=3"

export CONFIG="$HOME/.config"
export NVIM="$CONFIG/nvim"

# direnv is cross-platform; only hook it if installed.
command -v direnv >/dev/null 2>&1 && eval "$(direnv hook zsh)"
