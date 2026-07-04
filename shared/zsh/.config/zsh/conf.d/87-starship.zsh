# Config lives in ~/.config/starship.toml
command -v starship >/dev/null || return 0
eval "$(starship init zsh)"
