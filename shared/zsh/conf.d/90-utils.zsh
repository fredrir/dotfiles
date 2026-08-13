git() {
  case "$1:$#" in
    diff:1) command lazygit ;;
    log:1) command lazygit log ;;
    *) command git "$@" ;;
  esac
}

# Copy one path between matching locations under the home directories on
# macie and archie.
_home_copy() {
  emulate -L zsh

  local direction=$1
  local command_name="h$direction"
  shift

  local local_name remote_host remote_home
  case "$OSTYPE" in
    darwin*)
      local_name=macie
      remote_host=archie
      remote_home=/home/fredrir
      ;;
    linux*)
      local_name=archie
      remote_host=macie
      remote_home=/Users/fredrir
      ;;
    *)
      print -u2 -r -- "$command_name: unsupported operating system: $OSTYPE"
      return 1
      ;;
  esac

  local preview=0
  local use_excludes=1
  local -a rsync_args=(-aiR)
  local usage="usage: $command_name [-n|--dry-run] [-c|--checksum] [--all] path"
  local help="$usage

Copy a file or directory between the same home-relative location on macie
and archie. A relative path is resolved from the current directory.

  hpull  other machine -> this machine
  hpush  this machine  -> other machine

Files at the destination may be updated. Files absent from the source are
not deleted.

Options:
  -n, --dry-run   Preview what would be transferred
  -c, --checksum  Compare file contents instead of size and modification time
  --all           Include files matched by ~/.config/rsync/excludes
  -h, --help      Show this help

Examples:
  cd ~ && hpush .tmux.conf
      macie:~/.tmux.conf -> archie:~/.tmux.conf  (when run on macie)

  cd ~/projects/my-app && hpull project.yml
      Pull the matching ~/projects/my-app/project.yml from the other machine

  hpush --dry-run go
      Preview the copy and make no changes

Run the command to see the exact FROM and TO paths before confirming."

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
        print -r -- "$help"
        return 0
        ;;
      --)
        shift
        break
        ;;
      -*)
        print -u2 -r -- "$command_name: unknown option: $1"
        print -u2 -r -- "$usage"
        return 2
        ;;
      *)
        break
        ;;
    esac
    shift
  done

  (( $# == 1 )) || {
    print -u2 -r -- "$usage"
    return 2
  }

  local input_path=$1
  local local_path
  case "$input_path" in
    '~')
      local_path=$HOME
      ;;
    '~/'*)
      local_path="$HOME/${input_path#\~/}"
      ;;
    *)
      local_path=$input_path
      ;;
  esac

  local home_path=${HOME:A}
  local_path=${local_path:A}
  if [[ "$local_path" != "$home_path"/* ]]; then
    print -u2 -r -- "$command_name: path must be inside your home directory: $local_path"
    return 2
  fi
  local rel_path=${local_path#$home_path/}

  (( $+commands[rsync] )) || {
    print -u2 -r -- "$command_name: rsync is not installed"
    return 127
  }
  (( $+commands[ssh] )) || {
    print -u2 -r -- "$command_name: ssh is not installed"
    return 127
  }

  local config_home=${XDG_CONFIG_HOME:-"$HOME/.config"}
  local exclude_file="$config_home/rsync/excludes"
  if (( use_excludes )) && [[ -r "$exclude_file" ]]; then
    rsync_args+=(--exclude-from="$exclude_file")
  fi

  local suffix=''
  (( preview )) && suffix=' (dry run)'

  if [[ "$direction" == pull ]]; then
    print -u2 -r -- "$command_name$suffix"
    print -u2 -r -- "  FROM  $remote_host:$remote_home/$rel_path"
    print -u2 -r -- "  TO    $local_name:$local_path"
  else
    if [[ ! -e "$local_path" && ! -L "$local_path" ]]; then
      print -u2 -r -- "$command_name: local source does not exist: $local_path"
      return 1
    fi

    print -u2 -r -- "$command_name$suffix"
    print -u2 -r -- "  FROM  $local_name:$local_path"
    print -u2 -r -- "  TO    $remote_host:$remote_home/$rel_path"
  fi

  if (( ! preview )); then
    local reply
    while true; do
      if ! read -r "reply?Continue? [Y/n] "; then
        print
        return 1
      fi
      case "${reply:l}" in
        ''|y|yes)
          break
          ;;
        n|no)
          print -r -- "$command_name: cancelled"
          return 0
          ;;
        *)
          print -u2 -r -- 'Please answer y or n.'
          ;;
      esac
    done
  fi

  # --relative preserves rel_path and creates missing destination directories.
  if [[ "$direction" == pull ]]; then
    command rsync "${rsync_args[@]}" -- \
      "$remote_host:${(q)rel_path}" \
      "$home_path/"
  else
    (
      builtin cd -- "$home_path" || return
      command rsync "${rsync_args[@]}" -- \
        "$rel_path" \
        "$remote_host:"
    )
  fi
}

hpull() {
  _home_copy pull "$@"
}

hpush() {
  _home_copy push "$@"
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
