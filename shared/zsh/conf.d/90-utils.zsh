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
  local rel_path=${local_path#"$home_path"/}

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

# Which route ssh will take, and whether a master is already up for it.
#
# Every failure in the cabled-first setup degrades quietly to a working but
# much slower path: nc missing, cable unplugged, no DHCP lease yet, sshd
# restarting. All of them look identical from the outside, which is that
# things still work. This resolves the config the same way the next ssh will,
# without opening a connection, so the fallback stops being invisible.
hpath() {
  emulate -L zsh

  local usage="usage: hpath [--json] [host ...]"
  local help="$usage

Report the route ssh would take for a host without connecting to it. The
Match exec probe in ~/.ssh/config.d/05-* runs during config resolution, so
this is the same decision the next \`ssh <host>\` will make.

With no host, reports the other machine: archie from macie, macie from archie.

The route column reads whether ssh will bind the cabled interface for this
host, which is what separates the two routes to archie and macie. A host with
no cabled variant always reports tailscale.

Options:
      --json  Machine-readable
  -h, --help  Show this help

See also: hwire, which measures what a route is actually worth."

  local json=0
  while (( $# )); do
    case "$1" in
      --json)
        json=1
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
        print -u2 -r -- "hpath: unknown option: $1"
        print -u2 -r -- "$usage"
        return 2
        ;;
      *)
        break
        ;;
    esac
    shift
  done

  (( $+commands[ssh] )) || {
    print -u2 -r -- "hpath: ssh is not installed"
    return 127
  }

  local -a hosts=("$@")
  if (( ! $#hosts )); then
    case "$OSTYPE" in
      darwin*) hosts=(archie) ;;
      linux*) hosts=(macie) ;;
      *)
        print -u2 -r -- "hpath: unsupported operating system: $OSTYPE"
        return 1
        ;;
    esac
  fi

  # All declared up here: re-running `local` on an existing local inside the
  # loop makes zsh print the parameter instead of quietly redeclaring it.
  local exit_status=0
  local -a resolved names targets routes masters
  local host line key value hostname bound master route route_id master_json

  for host in "${hosts[@]}"; do
    resolved=(${(f)"$(ssh -G "$host" 2>/dev/null)"})
    if (( ! $#resolved )); then
      print -u2 -r -- "hpath: no config resolved for $host"
      exit_status=1
      continue
    fi

    hostname=
    bound=
    for line in "${resolved[@]}"; do
      key=${line%% *}
      value=${line#* }
      case "$key" in
        hostname) hostname=$value ;;
        bindinterface) bound=$value ;;
        bindaddress) [[ -n "$bound" ]] || bound=$value ;;
      esac
    done

    # BindInterface and BindAddress are only ever set on the cabled routes,
    # so their presence is what separates the two paths in this config.
    if [[ -n "$bound" ]]; then
      route="cabled via $bound"
      route_id=cable
    else
      route=tailscale
      route_id=tailscale
    fi

    if ssh -O check "$host" >/dev/null 2>&1; then
      master=up
      master_json=true
    else
      master=none
      master_json=false
    fi

    if (( json )); then
      printf '{"host":"%s","hostname":"%s","route":"%s","bound":"%s","master":%s}\n' \
        "$host" "$hostname" "$route_id" "$bound" "$master_json"
    else
      names+=("$host")
      targets+=("$hostname")
      routes+=("$route")
      masters+=("$master")
    fi
  done

  if (( ! json )) && (( $#names )); then
    local -i name_width=0 target_width=0 route_width=0
    local field
    for field in "${names[@]}"; do (( $#field > name_width )) && name_width=$#field; done
    for field in "${targets[@]}"; do (( $#field > target_width )) && target_width=$#field; done
    for field in "${routes[@]}"; do (( $#field > route_width )) && route_width=$#field; done

    local -i i
    for (( i = 1; i <= $#names; i++ )); do
      printf '%-*s  %-*s  %-*s  master %s\n' \
        "$name_width" "$names[i]" \
        "$target_width" "$targets[i]" \
        "$route_width" "$routes[i]" \
        "$masters[i]"
    done
  fi

  return $exit_status
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
      print -ru2 -- "cd: ambiguous case-insensitive match: ${matches[*]}"
      return 1
      ;;
  esac
}

alias cd='nocorrect cd'
