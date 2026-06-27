export SSH_AUTH_SOCK="$XDG_RUNTIME_DIR/keyring/ssh"
export NVM_DIR="$HOME/.config/nvm"
export PAGER=less
export LESS="-R -F --mouse --wheel-lines=3"

export CONFIG="$HOME/.config"
export HYPR="$CONFIG/hypr"
export NVIM="$CONFIG/nvim"

export PATH="$HOME/.local/share/npm-global/bin:$PATH"

eval "$(direnv hook zsh)"
