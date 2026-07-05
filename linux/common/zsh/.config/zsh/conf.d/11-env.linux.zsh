export SSH_AUTH_SOCK="$XDG_RUNTIME_DIR/keyring/ssh"

export PATH="$HOME/.local/share/npm-global/bin:$PATH"
export PATH=/home/fredrir/.opencode/bin:$PATH

sudo() {
  local dir="${PWD/#$HOME/~}"
  local g=$'\e[1;32m' x="${THEME_RESET}"

  command sudo -p "${g}[${dir}]${x}${THEME_SUDO}[sudo]${x}${g}\$${x} " "$@"
}
