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

  IFS= read -r -d '' cwd <"$cwd_file"
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

  IFS= read -r -d '' target <"$target_file"
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
  local -a matches=(${~pattern}(N-/))

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

ipp() {
  local include_ipv4=1
  local include_ipv6=1
  local os=$(uname -s)

  while (($#)); do
    case "$1" in
    -4)
      include_ipv4=1
      include_ipv6=0
      ;;
    -6)
      include_ipv4=0
      include_ipv6=1
      ;;
    esac
    shift
  done

  if [[ $os == "Darwin" ]]; then
    local i ipv4 ipv6_raw
    local -a ipv6

    for i in $(ifconfig -l); do
      ipv4=""
      ipv6_raw=""
      ipv6=()

      if ((include_ipv4)); then
        ipv4=$(ipconfig getifaddr "$i" 2>/dev/null)
      fi

      if ((include_ipv6)); then
        ipv6_raw=$(ifconfig "$i" 2>/dev/null |
          awk '/inet6 / {print $2}')

        if [[ -n $ipv6_raw ]]; then
          ipv6=("${(@f)ipv6_raw}")
        fi
      fi

      if [[ -n $ipv4 ]]; then
        printf "%-10s IPv4  %s\n" "$i" "$ipv4"
      fi

      if (($#ipv6)); then
        printf "%-10s IPv6  %s\n" "$i" "${(j: | :)ipv6}"
      fi
    done

  elif [[ $os == "Linux" ]]; then
    local i ipv4_raw ipv6_raw
    local -a interfaces ipv4 ipv6

    interfaces=("${(@f)$(ip -o link show |
      awk -F': ' '{sub(/@.*/, "", $2); print $2}')}")

    for i in "${interfaces[@]}"; do
      ipv4_raw=""
      ipv6_raw=""
      ipv4=()
      ipv6=()

      if ((include_ipv4)); then
        ipv4_raw=$(ip -o -4 addr show dev "$i" 2>/dev/null |
          awk '{sub(/\/.*/, "", $4); print $4}')

        if [[ -n $ipv4_raw ]]; then
          ipv4=("${(@f)ipv4_raw}")
        fi
      fi

      if ((include_ipv6)); then
        ipv6_raw=$(ip -o -6 addr show dev "$i" 2>/dev/null |
          awk '{sub(/\/.*/, "", $4); print $4}')

        if [[ -n $ipv6_raw ]]; then
          ipv6=("${(@f)ipv6_raw}")
        fi
      fi

      if (($#ipv4)); then
        printf "%-10s IPv4  %s\n" "$i" "${(j: | :)ipv4}"
      fi

      if (($#ipv6)); then
        printf "%-10s IPv6  %s\n" "$i" "${(j: | :)ipv6}"
      fi
    done
  fi
}

doppler-refresh() {
  local root=${DIRENV_DIR#-}

  if [[ -z $root ]]; then
    print -ru2 -- "doppler-refresh: no direnv environment loaded here"
    return 1
  fi

  command rm -f -- "$root"/.direnv/doppler.*.enc.json(N)
  direnv reload
}

inspect-port() {
  sudo lsof -n -i :"$1" | grep LISTEN
}

inspect-pid() {
  ps -p "$1" -o pid,vsz=MEMORY -o user,group=GROUP -o comm,args=ARGS
}
