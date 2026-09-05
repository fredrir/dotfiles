if [[ -z ${XDG_RUNTIME_DIR:-} && -d /run/user/$EUID && -O /run/user/$EUID ]]; then
  export XDG_RUNTIME_DIR="/run/user/$EUID"
fi

if [[ -z ${DBUS_SESSION_BUS_ADDRESS:-} && -S ${XDG_RUNTIME_DIR:-}/bus ]]; then
  export DBUS_SESSION_BUS_ADDRESS="unix:path=$XDG_RUNTIME_DIR/bus"
fi

export SSH_AUTH_SOCK="$XDG_RUNTIME_DIR/keyring/ssh"
# Test
export SUDO_EDITOR="/usr/bin/nvim"

export PATH="$HOME/.local/share/npm-global/bin:$PATH"
export PATH=/home/fredrir/.opencode/bin:$PATH
export PATH="/home/fredrir/.bun/bin:$PATH"

sudo() {
  command sudo -p "${THEME_SUDO}SUDO${THEME_CHAR}\$${THEME_RESET} " "$@"
}
