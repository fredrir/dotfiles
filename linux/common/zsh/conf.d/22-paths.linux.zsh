typeset -U path PATH
path=("${(@)path:#$HOME/dotfiles/scripts/.venv/bin}")

path=(
  "/opt/IntelliJ/bin"
  "/opt/pycharm/bin"
  "$HOME/.local/bin"
  "$HOME/.cargo/bin"
  $path
)

export PATH
