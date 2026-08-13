git() {
  case "$1:$#" in
    diff:1) command lazygit ;;
    log:1) command lazygit log ;;
    *) command git "$@" ;;
  esac
}

# Copy one home-relative file or directory between this machine and a remote.
hcopy() {
  emulate -L zsh

  local preview=0
  local use_excludes=1
  local -a rsync_args=(-ai)
  local usage='usage: hcopy <pull|push> [-n|--dry-run] [-c|--checksum] [--all] host home-relative-path'
  local help="$usage

Copy one path between matching locations in the local and remote home
directories. The direction is always stated explicitly:

  pull  remote -> local
  push  local  -> remote

Files at the destination may be updated. Files absent from the source are
not deleted.

Options:
  -n, --dry-run   Preview what would be transferred
  -c, --checksum  Compare file contents instead of size and modification time
  --all           Include files matched by ~/.config/rsync/excludes
  -h, --help      Show this help

Examples:
  hcopy pull archie .config/nvim
      FROM  archie:~/.config/nvim
      TO    local:~/.config/nvim

  hcopy push archie Documents/notes.md
      FROM  local:~/Documents/notes.md
      TO    archie:~/Documents/notes.md

  hcopy pull --dry-run archie projects/my-app
      Preview a remote-to-local copy without changing either machine

  hcopy push --checksum user@example.com projects/my-app
      Copy local changes to the remote, comparing files by content

  hcopy push --all archie projects/my-app
      Include normally excluded files such as .git and __pycache__"

  case "${1:-}" in
    pull|push)
      local direction=$1
      shift
      ;;
    -h|--help|help)
      print -r -- "$help"
      return 0
      ;;
    '')
      print -u2 -r -- "$usage"
      return 2
      ;;
    *)
      print -u2 -r -- "hcopy: direction must be 'pull' or 'push': $1"
      print -u2 -r -- "$usage"
      return 2
      ;;
  esac

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
        print -u2 -r -- "hcopy: unknown option: $1"
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
      print -u2 -r -- 'hcopy: host must be an SSH name or user@host'
      return 2
      ;;
  esac

  case "$rel_path" in
    ''|.|./*|*/./*|*/.|/*|~*|..|../*|*/../*|*/..)
      print -u2 -r -- 'hcopy: path must be clean and relative to the home directory'
      return 2
      ;;
  esac

  (( $+commands[rsync] )) || {
    print -u2 -r -- 'hcopy: rsync is not installed'
    return 127
  }
  (( $+commands[ssh] )) || {
    print -u2 -r -- 'hcopy: ssh is not installed'
    return 127
  }

  local config_home=${XDG_CONFIG_HOME:-"$HOME/.config"}
  local exclude_file="$config_home/rsync/excludes"
  if (( use_excludes )) && [[ -r "$exclude_file" ]]; then
    rsync_args+=(--exclude-from="$exclude_file")
  fi

  local label=$direction
  (( preview )) && label="$label (dry run)"

  if [[ "$direction" == pull ]]; then
    print -u2 -r -- "hcopy $label"
    print -u2 -r -- "  FROM  $remote_host:~/$rel_path"
    print -u2 -r -- "  TO    local:~/$rel_path"

    # Copying into the source's parent preserves the same path for both files
    # and directories without changing metadata on the home directory itself.
    local local_parent="$HOME/${rel_path:h}"
    if [[ ! -d "$local_parent" ]]; then
      if (( preview )); then
        print -u2 -r -- "hcopy: local destination parent does not exist: $local_parent"
        print -u2 -r -- 'hcopy: a real pull would create it'
        return 1
      fi
      command mkdir -p -- "$local_parent" || return
    fi

    command rsync "${rsync_args[@]}" -- \
      "$remote_host:${(q)rel_path}" \
      "$local_parent/"
  else
    print -u2 -r -- "hcopy $label"
    print -u2 -r -- "  FROM  local:~/$rel_path"
    print -u2 -r -- "  TO    $remote_host:~/$rel_path"

    local local_source="$HOME/$rel_path"
    if [[ ! -e "$local_source" && ! -L "$local_source" ]]; then
      print -u2 -r -- "hcopy: local source does not exist: $local_source"
      return 1
    fi

    local remote_parent=${rel_path:h}
    if [[ "$remote_parent" != . ]]; then
      if (( preview )); then
        if ! command ssh "$remote_host" "test -d ${(q)remote_parent}"; then
          print -u2 -r -- "hcopy: remote destination parent does not exist: $remote_host:~/$remote_parent"
          print -u2 -r -- 'hcopy: a real push would create it'
          return 1
        fi
      else
        command ssh "$remote_host" "mkdir -p -- ${(q)remote_parent}" || return
      fi
    fi

    command rsync "${rsync_args[@]}" -- \
      "$local_source" \
      "$remote_host:${(q)remote_parent}/"
  fi
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
