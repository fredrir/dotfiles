# ~/.zshenv
export DOTFILES="$HOME/dotfiles"
export DOTFILES_CONFIG="$DOTFILES/config"
export DOTFILES_BIN="$DOTFILES/scripts/shell"
export DOTFILES_ZSH_CACHE="$DOTFILES/.cache/zsh"
export DOTFILES_COMPILED="$DOTFILES/.bin"

export ZCONF="$DOTFILES/shared/zsh"

[[ -d "$HOME/llama.cpp" ]] && export LLAMA="$HOME/llama.cpp"

# zsh-build: omit
[[ -o interactive && -o global_rcs && -z $ZSH_BUILD_SKIP && -r $DOTFILES_ZSH_CACHE/build/zshrc.rcs.zsh ]] && source "$DOTFILES_ZSH_CACHE/build/zshrc.rcs.zsh"
