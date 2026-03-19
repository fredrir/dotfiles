source "$HOME/dotfiles/zsh/conf.d/05-ohmyzsh.zsh"
source $ZSH/oh-my-zsh.sh

for file in "$HOME"/dotfiles/zsh/conf.d/*.zsh; do
  [[ -f "$file" ]] && source "$file"
done
