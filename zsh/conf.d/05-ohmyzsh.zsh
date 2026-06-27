export ZSH="$HOME/.oh-my-zsh"
ZSH_THEME="eastwood"

# plugins must be set BEFORE `source $ZSH/oh-my-zsh.sh` (which .zshrc does
# right after sourcing this file), otherwise oh-my-zsh never loads them.
plugins=(git)

zstyle ':omz:update' mode reminder
COMPLETION_WAITING_DOTS="true"
