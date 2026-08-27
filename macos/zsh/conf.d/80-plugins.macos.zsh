_brew_share="${HOMEBREW_PREFIX:-/opt/homebrew}/share"

[[ -f "$_brew_share/fzf-tab/fzf-tab.zsh" ]] && source "$_brew_share/fzf-tab/fzf-tab.zsh"

[[ -f "$_brew_share/zsh-syntax-highlighting/zsh-syntax-highlighting.zsh" ]] &&
  source "$_brew_share/zsh-syntax-highlighting/zsh-syntax-highlighting.zsh"

# Ghost text of the previously typed command.
ZSH_AUTOSUGGEST_STRATEGY=(history completion)
[[ -f "$_brew_share/zsh-autosuggestions/zsh-autosuggestions.zsh" ]] &&
  source "$_brew_share/zsh-autosuggestions/zsh-autosuggestions.zsh"

unset _brew_share
