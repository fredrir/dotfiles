# nvm (cross-platform). NVM_DIR is set here so it is correct everywhere —
# note it is ~/.config/nvm, NOT ~/.config/nvim (a past typo caused nvm to
# install into the neovim config dir).
export NVM_DIR="$HOME/.config/nvm"
[ -s "$NVM_DIR/nvm.sh" ] && source "$NVM_DIR/nvm.sh"
[ -s "$NVM_DIR/bash_completion" ] && source "$NVM_DIR/bash_completion"
