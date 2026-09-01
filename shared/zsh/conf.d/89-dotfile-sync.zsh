dotfile() {
  command dotfile "$@"
  local exit_status=$?
  ((exit_status == 0)) && rehash
  return exit_status
}
