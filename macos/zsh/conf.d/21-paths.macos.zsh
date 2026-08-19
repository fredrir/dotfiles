typeset -U path PATH
path=("${(@)path:#$HOME/dotfiles/scripts/.venv/bin}")

path=(
  "$HOME/.local/bin"
  "$HOME/.cargo/bin"
  $path
)

export PATH

# Bun
export BUN_INSTALL="$HOME/.bun"
export PATH="$BUN_INSTALL/bin:$PATH"
[ -s "$HOME/.bun/_bun" ] && source "$HOME/.bun/_bun"

export PATH="$PATH:/Applications/PyCharm.app/Contents/MacOS"

# opencode
export PATH=/Users/fredrir/.opencode/bin:$PATH

# Ruby
export PATH="/opt/homebrew/lib/ruby/gems/4.0.0/bin:$PATH"
