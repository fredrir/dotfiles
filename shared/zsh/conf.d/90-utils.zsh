git() {
  case "$1:$#" in
    diff:1) command lazygit ;;
    log:1) command lazygit log ;;
    *) command git "$@" ;;
  esac
}

# Pull one remote home-relative file or directory into the matching local path.
rpull() {
  emulate -L zsh

  local preview=0
  local use_excludes=1
  local -a rsync_args=(-ai)
  local usage='usage: rpull [-n|--dry-run] [-c|--checksum] [--all] host home-relative-path'

  while (( $# )); do
    case "$1" in
      -n|--dry-run)
        preview=1
        rsync_args+=(-n)
        ;;
      -c|--checksum)
        rsync_args+=(-c)
        ;;
      --all|--no-excludes)
        use_excludes=0
        ;;
      -h|--help)
        print -r -- "$usage"
        return 0
        ;;
      --)
        shift
        break
        ;;
      -*)
        print -u2 -r -- "rpull: unknown option: $1"
        print -u2 -r -- "$usage"
        return 2
        ;;
      *)
        break
        ;;
    esac
    shift
  done

  (( $# == 2 )) || {
    print -u2 -r -- "$usage"
    return 2
  }

  local remote_host=$1
  local rel_path=${2%/}

  case "$remote_host" in
    ''|-*|*[!A-Za-z0-9._@-]*)
      print -u2 -r -- 'rpull: host must be an SSH name or user@host'
      return 2
      ;;
  esac

  case "$rel_path" in
    ''|.|./*|*/./*|*/.|/*|~*|..|../*|*/../*|*/..)
      print -u2 -r -- 'rpull: path must be a clean path relative to the remote home'
      return 2
      ;;
  esac

  (( $+commands[rsync] )) || {
    print -u2 -r -- 'rpull: rsync is not installed'
    return 127
  }

  local config_home=${XDG_CONFIG_HOME:-"$HOME/.config"}
  local exclude_file="$config_home/rsync/excludes"
  if (( use_excludes )) && [[ -r "$exclude_file" ]]; then
    rsync_args+=(--exclude-from="$exclude_file")
  fi

  # Keeping the destination at the source's local parent makes this work for
  # both files and directories without changing metadata on $HOME itself.
  local local_parent="$HOME/${rel_path:h}"
  if [[ ! -d "$local_parent" ]]; then
    if (( preview )); then
      print -u2 -r -- "rpull: local parent does not exist: $local_parent"
      return 1
    fi
    command mkdir -p -- "$local_parent" || return
  fi

  command rsync "${rsync_args[@]}" -- \
    "$remote_host:${(q)rel_path}" \
    "$local_parent/"
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
