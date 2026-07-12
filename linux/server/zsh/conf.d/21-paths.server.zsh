# Server PATH — no GUI/IDE dirs, just the per-user tool locations the
# bootstrap installs into (neovim tarball, starship, cargo, uv/pip --user).
path=(
  "$HOME/.local/nvim/bin"
  "$HOME/.local/bin"
  "$HOME/.cargo/bin"
  $path
)

export PATH
