export CONFIG="$HOME/.config"
export DISABLE_MAGIC_FUNCTIONS=true
export NVM_DIR="$HOME/.config/nvm"
export ZSH="$HOME/.oh-my-zsh"
export ZSH_AUTOSUGGEST_STRATEGY=(history)
# hport serves this host to the peer as http://$HOST:PORT
export __VITE_ADDITIONAL_SERVER_ALLOWED_HOSTS=$HOST

has_cmd less &&
  export PAGER=less \
    export LESS="-R -F --mouse --wheel-lines=3"
has_cmd nvim &&
  export MANPAGER='nvim +Man!' \
    export MANWIDTH=999
has_cmd nvim &&
  export NVIM="$DOTFILES/shared/nvim" \
    export EDITOR=nvim \
    export SUDO_EDITOR="$(command -v nvim)"
