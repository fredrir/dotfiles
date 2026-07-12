export SSH_AUTH_SOCK="$XDG_RUNTIME_DIR/keyring/ssh"
# Test
export SUDO_EDITOR="/usr/bin/nvim"


export PATH="$HOME/.local/share/npm-global/bin:$PATH"
export PATH=/home/fredrir/.opencode/bin:$PATH
export PATH="/home/fredrir/.bun/bin:$PATH"

sudo() {
  command sudo -p "${THEME_SUDO}SUDO${THEME_CHAR}\$${THEME_RESET} " "$@"
}
