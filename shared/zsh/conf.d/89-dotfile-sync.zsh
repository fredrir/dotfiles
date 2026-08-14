dotfile() {
  if (( $# == 1 )) && [[ $1 == sync ]]; then
    command dotfile "$@" || return
    exec zsh
  fi

  command dotfile "$@"
}
