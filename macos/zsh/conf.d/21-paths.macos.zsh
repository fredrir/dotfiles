path=(
  "$HOME/.local/bin"
  "$HOME/dotfiles/scripts/.venv/bin"
  $path
)

export PATH

# Bun
export BUN_INSTALL="$HOME/.bun"
export PATH="$BUN_INSTALL/bin:$PATH"
[ -s "$HOME/.bun/_bun" ] && source "$HOME/.bun/_bun"

export PATH="$PATH:/Applications/PyCharm.app/Contents/MacOS"
