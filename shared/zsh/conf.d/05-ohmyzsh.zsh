export ZSH="$HOME/.oh-my-zsh"

# Fallback theme
ZSH_THEME="eastwood"

export NVM_DIR="$HOME/.config/nvm"
zstyle ':omz:plugins:nvm' lazy yes

# eza
zstyle ':omz:plugins:eza' dirs-first yes
zstyle ':omz:plugins:eza' git-status yes
zstyle ':omz:plugins:eza' icons yes

# Shell-only plugins — no external tool to check for.
plugins=(
  git
  gitignore

  alias-finder
  colored-man-pages
  copyfile
  copypath
)

(( $+commands[gh] ))      && plugins+=(gh)
(( $+commands[npm] ))     && plugins+=(npm)
(( $+commands[bun] ))     && plugins+=(bun)
[[ -d "$NVM_DIR" ]]       && plugins+=(nvm)
(( $+commands[docker] ))  && plugins+=(docker docker-compose)
(( $+commands[kubectl] )) && plugins+=(kubectl)
(( $+commands[helm] ))    && plugins+=(helm)
(( $+commands[psql] ))    && plugins+=(postgres)
(( $+commands[fzf] ))     && plugins+=(fzf)
(( $+commands[zoxide] ))  && plugins+=(zoxide)
(( $+commands[eza] ))     && plugins+=(eza)

if (( $+commands[fzf] )) && [[ -d "$ZSH/custom/plugins/fzf-tab" ]]; then
  plugins+=(fzf-tab)
elif (( $+commands[fzf] )) && [[ -f "${HOMEBREW_PREFIX:-/opt/homebrew}/share/fzf-tab/fzf-tab.zsh" ]]; then
  : # sourced by 80-plugins.macos.zsh, after compinit
else
  COMPLETION_WAITING_DOTS="true"
fi

[[ -d "$ZSH/custom/plugins/zsh-autosuggestions" ]]     && plugins+=(zsh-autosuggestions)
[[ -d "$ZSH/custom/plugins/zsh-syntax-highlighting" ]] && plugins+=(zsh-syntax-highlighting)

zstyle ':omz:update' mode reminder
