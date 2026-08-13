ssa() {
  emulate -L zsh

  local host=archie
  local action=attach
  local session=main
  local usage='usage: ssa [SESSION | --cc [SESSION] | ls | -ls | --list | delete SESSION | -rm SESSION]'

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
        command ssh "$host" 'tmux list-sessions'
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
        print -u2 -r -- "ssa: unknown option: $1"
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
      print -u2 -r -- 'ssa: session names may contain letters, numbers, _ and -'
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
    command ssh "$host" "tmux kill-session -t ${(q)target}"
  elif [[ "$action" == control ]]; then
    command ssh -t "$host" \
      "exec tmux -CC new-session -A -s ${(q)session}"
  else
    command ssh -t "$host" \
      "exec tmux new-session -A -s ${(q)session}"
  fi
}
