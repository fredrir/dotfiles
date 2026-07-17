# Count items inside a directory
# Options:
#   -r  recursive
#   -d  only non-hidden items
count() {
  local recursive=false
  local nonhidden=false
  local OPTIND opt

  while getopts ":rd" opt; do
    case "$opt" in
      r) recursive=true ;;
      d) nonhidden=true ;;
      *)
        echo "Usage: count [-r] [-d] <directory>" >&2
        return 1
        ;;
    esac
  done
  shift $((OPTIND - 1))

  if [[ -z "$1" ]]; then
    echo "Usage: count [-r] [-d] <directory>"
    return 1
  fi

  if [[ ! -d "$1" ]]; then
    echo "count: not a directory: $1" >&2
    return 1
  fi

  if $recursive; then
    if $nonhidden; then
      find "$1" -mindepth 1 -not -path '*/.*' | wc -l
    else
      find "$1" -mindepth 1 | wc -l
    fi
  else
    if $nonhidden; then
      find "$1" -mindepth 1 -maxdepth 1 -not -name '.*' | wc -l
    else
      find "$1" -mindepth 1 -maxdepth 1 | wc -l
    fi
  fi
}

# Show total size of a file or directory
# Options:
#   -d  only non-hidden items when given a directory
size() {
  local nonhidden=false
  local OPTIND opt

  while getopts ":d" opt; do
    case "$opt" in
      d) nonhidden=true ;;
      *)
        echo "Usage: size [-d] <path>" >&2
        return 1
        ;;
    esac
  done
  shift $((OPTIND - 1))

  if [[ -z "$1" ]]; then
    echo "Usage: size [-d] <path>"
    return 1
  fi

  if [[ ! -e "$1" ]]; then
    echo "size: no such file or directory: $1" >&2
    return 1
  fi

  if $nonhidden && [[ -d "$1" ]]; then
    find "$1" -mindepth 1 -maxdepth 1 -not -name '.*' -exec du -sch {} + \
      | awk '/total$/ {print $1}'
  else
    du -sh "$1" | cut -f1
  fi
}

git() {
  if [[ "$1" == "diff" && $# -eq 1 ]]; then
    command lazygit
  else
    command git "$@"
  fi
}

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


oc() {
  ss -Htln 'sport = :18789' 2>/dev/null | grep -q . \
    || ssh -f -N -o ExitOnForwardFailure=yes -o ServerAliveInterval=30 \
         -L 18789:127.0.0.1:18789 hetzner
  if [ $# -gt 0 ]; then
    openclaw agent --agent main --message "$*"
  else
    openclaw tui
  fi
}


unalias gdd 2>/dev/null
unfunction gdd 2>/dev/null

gdd() {
  if (( $# == 0 )); then
    echo "Usage: gdd <commit message>"
    return 1
  fi

  git add . || return 1

  if ! git diff --cached --quiet; then
    git commit -m "$*" || return 1
  else
    echo "Nothing new to commit; pushing existing commits."
  fi

  git push
}
