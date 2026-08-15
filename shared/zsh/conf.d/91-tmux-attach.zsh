# Attach the peer machine's terminal from either side.
#
#   ssa   archie      ssm   macie
#
# Inside wezterm with the USB link up, a bare attach opens a native mux tab
# (wezterm domain archie-usb / macie-usb): panes live on the peer's mux
# server and survive the cable. Everything else — explicit sessions, --tmux,
# other terminals, Tailscale — uses tmux over ssh. Whichever host you are
# already on resolves to the local server rather than an ssh loop back.
_usb_link_up() {
  case "$OSTYPE" in
    darwin*) command nc -4 -z -G 1 -b en3 10.77.77.2 22 >/dev/null 2>&1 ;;
    linux*) command nc -z -w 1 -s 10.77.77.2 10.77.77.1 22 >/dev/null 2>&1 ;;
    *) return 1 ;;
  esac
}

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
  local explicit_session=0
  local force_tmux=0
  local usage="usage: $command_name [SESSION | --tmux [SESSION] | ls | delete SESSION]"

  if (( $# )); then
    case "$1" in
      -t|--tmux)
        (( $# <= 2 )) || {
          print -u2 -r -- "$usage"
          return 2
        }
        force_tmux=1
        if (( $# == 2 )); then
          session=$2
          explicit_session=1
        fi
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
        explicit_session=1
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
        explicit_session=1
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

  # Native wezterm path when nothing tmux-specific was asked for (tmux
  # sessions are a tmux concept, so a named session always means tmux).
  # Local host: a new tab in the current pane's domain — inside a mux pane
  # that domain IS the peer's mux server. Remote host: the -usb ssh domain
  # when the cable answers; otherwise say why tmux is taking over.
  if (( ! force_tmux && ! explicit_session )) && [[ -n "$WEZTERM_UNIX_SOCKET" ]]; then
    if (( local_server )); then
      command wezterm cli spawn >/dev/null 2>&1 && return 0
    elif _usb_link_up; then
      command wezterm cli spawn --domain-name "${host}-usb" >/dev/null 2>&1 && return 0
      print -u2 -r -- "$command_name: native mux spawn failed, falling back to tmux"
    else
      print -u2 -r -- "$command_name: usb link down, tmux over ssh instead"
    fi
  fi

  if (( local_server )); then
    command tmux new-session -A -s "$session"
  else
    command ssh -t "$host" "exec tmux new-session -A -s ${(q)session}"
  fi
}

ssa() { _tmux_attach archie ssa "$@" }
ssm() { _tmux_attach macie ssm "$@" }
