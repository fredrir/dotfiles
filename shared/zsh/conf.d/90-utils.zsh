git() {
  case "$1:$#" in
    diff:1) command lazygit ;;
    log:1) command lazygit log ;;
    *) command git "$@" ;;
  esac
}

unalias cd 2>/dev/null
cd() {
  if (( $# != 1 )) || [[ "$1" == -* ]] || [[ -d "$1" ]]; then
    builtin cd "$@"
    return
  fi

  setopt localoptions extendedglob

  local pattern="(#i)${(b)1}"
  local -a matches=( ${~pattern}(N-/) )

  case $#matches in
    1)
      builtin cd -- "$matches[1]"
      ;;
    0)
      builtin cd -- "$1"
      ;;
    *)
      print -u2 "cd: ambiguous case-insensitive match: ${matches[*]}"
      return 1
      ;;
  esac
}

alias cd='nocorrect cd'
