add_plugin_path \
  "${HOMEBREW_PREFIX:-/opt/homebrew}/share" \
  /usr/share/zsh/plugins \
  /usr/local/share

add_fpath \
  "$HOME/.zfunc" \
  "${HOMEBREW_PREFIX:-/opt/homebrew}/share/zsh-completions"


add_plugins \
  git \
  zsh:gitignore \
  zsh:alias-finder \
  zsh:colored-man-pages \
  zsh:copyfile \
  zsh:copypath \
  gh \
  npm \
  bun \
  zsh:nvm \
  kubectl \
  helm \
  psql:postgres \
  docker \
  docker:docker-compose \
  fzf \
  fzf:fzf-tab \
  zoxide \
  eza \
  brew \
  sw_vers:macos \
  pacman:archlinux \
  zsh:zsh-autosuggestions \
  zsh:zsh-syntax-highlighting

zstyle ':omz:update' mode reminder
zstyle ':omz:plugins:nvm' lazy yes
zstyle ':omz:plugins:eza' dirs-first yes
zstyle ':omz:plugins:eza' git-status yes
zstyle ':omz:plugins:eza' icons yes

[[ -f "$ZSH/oh-my-zsh.sh" ]] && source "$ZSH/oh-my-zsh.sh"
