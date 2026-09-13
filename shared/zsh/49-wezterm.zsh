mux() {
  local mux_socket="$HOME/.local/share/wezterm/localmux.sock"
  WEZTERM_UNIX_SOCKET="$mux_socket" wezterm cli --prefer-mux --no-auto-start "$@"
}

# TODO: Disallow connection when already connected
attach_mux() {
  emulate -L zsh

  local list=0
  if [[ $1 == -l || $1 == --list ]]; then
    list=1
    shift
  fi

  if ((list)); then
    mux-route --list $1
    return
  fi

  if [[ -n $TMUX ]]; then
    tmux-workspace host "$1" --pane "$TMUX_PANE"
    return
  fi

  if [[ -z $WEZTERM_PANE ]]; then
    print -ru2 'mux: not a wezterm pane'
    return 1
  fi
  local target=${1:-peer}
  case $target in
  archie | macie | peer) ;;
  *)
    print -ru2 "mux: unknown host: $target"
    return 1
    ;;
  esac

  zmodload zsh/datetime
  local request="v1:$target:$$:$EPOCHREALTIME"
  printf '\e]1337;SetUserVar=ATTACH_MUX=%s\a' "$(print -rn -- "$request" | base64 | tr -d '\r\n')"
}

[[ $HOST == "macie" ]] && alias archie='attach_mux archie'
[[ $HOST == "archie" ]] && alias macie='attach_mux macie'

if [[ -n $WEZTERM_PANE ]]; then
  WEZTERM_SHELL_SKIP_SEMANTIC_ZONES=1
  WEZTERM_SHELL_SKIP_CWD=1

  : ${WEZTERM_HOSTNAME:=$HOST}

  [[ -n $MACOS ]] && source "/Applications/WezTerm.app/Contents/Resources/wezterm.sh"
  [[ -n $LINUX ]] && source "/etc/profile.d/wezterm.sh"

  unset _wezterm_sh
fi
