puurge() {
  if (( $# != 1 )); then
    echo "usage: puurge <directory>" >&2
    return 2
  fi

  local dir="$1"

  if [[ ! -d "$dir" ]]; then
    echo "puurge: not a directory: $dir" >&2
    return 1
  fi

  find "$dir" -mindepth 1 -maxdepth 1 -print0 \
    | xargs -0 -n 1 -P 8 rm -rf --

  rmdir -- "$dir"
}