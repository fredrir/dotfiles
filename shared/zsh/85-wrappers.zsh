dotfile() {
  command dotfile "$@"
  local exit_status=$?
  ((exit_status == 0)) && rehash
  return exit_status
}

sudo() {
  command sudo -p "${THEME_SUDO}SUDO${THEME_CHAR}\$${THEME_RESET} " "$@"
}
