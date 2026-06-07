source "$HOME/dotfiles/zsh/conf.d/05-ohmyzsh.zsh"
source $ZSH/oh-my-zsh.sh

for file in "$HOME"/dotfiles/zsh/conf.d/*.zsh; do
  [[ -f "$file" ]] && source "$file"
done

. "$HOME/.local/bin/env"

export NVM_DIR="$HOME/.config/nvm"
[ -s "$NVM_DIR/nvm.sh" ] && \. "$NVM_DIR/nvm.sh"  # This loads nvm
[ -s "$NVM_DIR/bash_completion" ] && \. "$NVM_DIR/bash_completion"  # This loads nvm bash_completion
