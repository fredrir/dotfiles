git() {
  case "$1:$#" in
  diff:1) command lazygit ;;
  log:1) command lazygit log ;;
  *) command git "$@" ;;
  esac
}

lls() {
  local -a links=(*(ND@))

  ((${#links[@]})) || return 0

  eza -lh \
    --no-permissions \
    --no-filesize \
    --no-user \
    --no-time \
    -- "${links[@]}"
}

y() {
  local cwd cwd_file yazi_status
  cwd_file="$(mktemp -t 'yazi-cwd.XXXXXX')" || return

  command yazi "$@" --cwd-file="$cwd_file"
  yazi_status=$?

  IFS= read -r -d '' cwd < "$cwd_file"
  command rm -f -- "$cwd_file"

  if [[ -n "$cwd" && "$cwd" != "$PWD" && -d "$cwd" ]]; then
    builtin cd -- "$cwd"
  fi

  return "$yazi_status"
}

ycd() {
  local target target_file yazi_status
  target_file="$(mktemp -t 'yazi-chooser.XXXXXX')" || return

  command yazi "$@" --chooser-file="$target_file"
  yazi_status=$?

  IFS= read -r -d '' target < "$target_file"
  command rm -f -- "$target_file"

  if [[ -n "$target" && -d "$target" ]]; then
    builtin cd -- "$target"
  fi

  return "$yazi_status"
}

unalias cd 2>/dev/null
cd() {
  if (($# != 1)) || [[ "$1" == -* ]] || [[ -d "$1" ]]; then
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
    print -ru2 -- "cd: ambiguous case-insensitive match: ${matches[*]}"
    return 1
    ;;
  esac
}

alias cd='nocorrect cd'
