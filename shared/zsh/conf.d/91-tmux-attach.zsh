# Attach a tmux session on macie or archie from either machine.
#
#   ssa   tmux on archie      ssm   tmux on macie
#
# Whichever host you are already on resolves to the local tmux server rather
# than an ssh loop back to itself.
_tmux_attach() {
  emulate -L zsh

  local host=$1
  local command_name=$2
  shift 2

  local this_host
  case "$OSTYPE" in
    darwin*) this_host=macie ;;
    linux*) this_host=archie ;;
    *)
      print -u2 -r -- "$command_name: unsupported operating system: $OSTYPE"
      return 1
      ;;
  esac

  local local_server=0
  [[ "$host" == "$this_host" ]] && local_server=1

  local action=attach
  local session=main
  local usage="usage: $command_name [SESSION | --cc [SESSION] | ls | delete SESSION]"

  if (( $# )); then
    case "$1" in
      # Control mode: WezTerm renders the session's windows as native tabs.
      -cc|--cc|--control)
        (( $# <= 2 )) || {
          print -u2 -r -- "$usage"
          return 2
        }
        action=control
        (( $# == 2 )) && session=$2
        ;;
      ls|list|-l|-ls|--list)
        (( $# == 1 )) || {
          print -u2 -r -- "$usage"
          return 2
        }
        if (( local_server )); then
          command tmux list-sessions
        else
          command ssh "$host" 'tmux list-sessions'
        fi
        return
        ;;
      delete|rm|-rm|--delete)
        (( $# == 2 )) || {
          print -u2 -r -- "$usage"
          return 2
        }
        action=delete
        session=$2
        ;;
      -h|--help)
        print -r -- "$usage"
        return
        ;;
      --)
        shift
        (( $# == 1 )) || {
          print -u2 -r -- "$usage"
          return 2
        }
        session=$1
        ;;
      -*)
        print -u2 -r -- "$command_name: unknown option: $1"
        return 2
        ;;
      *)
        (( $# == 1 )) || {
          print -u2 -r -- "$usage"
          return 2
        }
        session=$1
        ;;
    esac
  fi

  case "$session" in
    ''|-*|*[!A-Za-z0-9_-]*)
      print -u2 -r -- "$command_name: session names may contain letters, numbers, _ and -"
      return 2
      ;;
  esac

  if [[ "$action" == delete ]]; then
    local reply
    if ! read -q "reply?Delete '$session' on '$host'? [y/N] "; then
      print
      return 1
    fi
    print

    # The leading '=' makes tmux use an exact session name, not a prefix match.
    local target="=$session"
    if (( local_server )); then
      command tmux kill-session -t "$target"
    else
      command ssh "$host" "tmux kill-session -t ${(q)target}"
    fi
    return
  fi

  local -a mode
  [[ "$action" == control ]] && mode=(-CC)

  if (( local_server )); then
    command tmux $mode new-session -A -s "$session"
  else
    command ssh -t "$host" "exec tmux ${mode:+-CC }new-session -A -s ${(q)session}"
  fi
}

ssa() { _tmux_attach archie ssa "$@" }
ssm() { _tmux_attach macie ssm "$@" }
