ZCONF="$HOME/.config/zsh/conf.d"

[[ -f "$ZCONF/05-ohmyzsh.zsh" ]] && source "$ZCONF/05-ohmyzsh.zsh"
[[ -f "$ZSH/oh-my-zsh.sh" ]] && source "$ZSH/oh-my-zsh.sh"

for file in "$ZCONF"/*.zsh; do
  [[ -f "$file" ]] && source "$file"
done

[[ -f "$HOME/.local/bin/env" ]] && source "$HOME/.local/bin/env"
