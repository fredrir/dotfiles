export SSH_AUTH_SOCK="$XDG_RUNTIME_DIR/keyring/ssh"
export SUDO_EDITOR="/usr/bin/nvim"


export PATH="$HOME/.local/share/npm-global/bin:$PATH"
export PATH=/home/fredrir/.opencode/bin:$PATH

sudo() {
  local dir="${PWD/#$HOME/~}" x="${THEME_RESET}"
  local branch git_seg=""
  branch="$(command git symbolic-ref --short HEAD 2>/dev/null)" \
    && git_seg="${THEME_GIT}[${branch}]${x}"

  command sudo -p "${git_seg}${THEME_DIR}[${dir}]${x}${THEME_SUDO}[sudo]${x}${THEME_CHAR}\$${x} " "$@"
}
