export CONFIG="$HOME/.config"
export DISABLE_MAGIC_FUNCTIONS=true
export NVM_DIR="$HOME/.config/nvm"
export ZSH="$HOME/.oh-my-zsh"
export ZSH_DISABLE_COMPFIX=true
export ZSH_AUTOSUGGEST_STRATEGY=(history)
export ZSH_AUTOSUGGEST_MANUAL_REBIND=1
export __VITE_ADDITIONAL_SERVER_ALLOWED_HOSTS=$HOST # hport: http://$HOST:PORT

has_cmd less &&
  export PAGER=less \
    LESS="-R -F --mouse --wheel-lines=3"
has_cmd hexcat &&
  export BAT_PAGER="hexcat --pager"
has_cmd nvim &&
  export MANPAGER='nvim +Man!' \
    MANWIDTH=999
has_cmd nvim &&
  export NVIM="$DOTFILES/shared/nvim" \
    EDITOR=nvim \
    SUDO_EDITOR="$(command -v nvim)"
