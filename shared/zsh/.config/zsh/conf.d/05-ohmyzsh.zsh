export ZSH="$HOME/.oh-my-zsh"

# Fallback theme
ZSH_THEME="eastwood"

export NVM_DIR="$HOME/.config/nvm"
zstyle ':omz:plugins:nvm' lazy yes

# eza
zstyle ':omz:plugins:eza' dirs-first yes
zstyle ':omz:plugins:eza' git-status yes
zstyle ':omz:plugins:eza' icons yes

plugins=(
  git
  gh
  gitignore

  # on-demand only (no autoload zstyle): `alias-finder "git add"`
  alias-finder
  colored-man-pages
  command-not-found
  copyfile
  copypath

  npm
  bun
  nvm

  docker
  docker-compose
  kubectl
  helm

  postgres
)

(( $+commands[fzf] ))    && plugins+=(fzf)
(( $+commands[zoxide] )) && plugins+=(zoxide)
(( $+commands[eza] ))    && plugins+=(eza)

if [[ -d "$ZSH/custom/plugins/fzf-tab" ]] && (( $+commands[fzf] )); then
  plugins+=(fzf-tab)
else
  COMPLETION_WAITING_DOTS="true"
fi

zstyle ':omz:update' mode reminder
