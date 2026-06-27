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
