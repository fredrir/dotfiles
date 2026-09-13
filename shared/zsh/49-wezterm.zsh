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
  export WEZTERM_SHELL_SKIP_SEMANTIC_ZONES=1
  export WEZTERM_SHELL_SKIP_CWD=1

  : ${WEZTERM_HOSTNAME:=$HOST}

  for _wezterm_sh in \
    /Applications/WezTerm.app/Contents/Resources/wezterm.sh \
    /etc/profile.d/wezterm.sh \
    /usr/share/wezterm/shell-integration/wezterm.sh; do
    [[ -r $_wezterm_sh ]] || continue
    source "$_wezterm_sh"
    break
  done
  unset _wezterm_sh
fi

[[ -o interactive ]] || return 0

_wezterm_open_yazi() {
  local cwd cwd_file yazi_status
  zle -I
  if [[ -n $TMUX ]] && has_cmd tmux-workspace; then
    cwd_file=$(mktemp -t 'tmux-yazi-cwd.XXXXXX') || return
    {
      tmux-workspace yazi --pane "$TMUX_PANE" --cwd-file "$cwd_file"
      yazi_status=$?
      IFS= read -r -d '' cwd <"$cwd_file"
      [[ -n $cwd && -d "$cwd" && "$cwd" != "$PWD" ]] && builtin cd -- "$cwd"
    } always {
      command rm -f -- "$cwd_file"
    }
  else
    ycd
    yazi_status=$?
  fi
  zle reset-prompt
  return "$yazi_status"
}

zle -N wezterm-open-yazi _wezterm_open_yazi