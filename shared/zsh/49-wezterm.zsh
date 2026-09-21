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

  if [[ -z $WEZTERM_PANE ]]; then
    print -ru2 'mux: not a wezterm pane'
    return 1
  fi
  local target=${1:-peer}
  if [[ $target == *[^[:alnum:]._-]* ]]; then
    print -ru2 "mux: invalid host: $target"
    return 1
  fi

  zmodload zsh/datetime
  local request="v1:$target:$$:$EPOCHREALTIME"
  printf '\e]1337;SetUserVar=ATTACH_MUX=%s\a' "$(print -rn -- "$request" | base64 | tr -d '\r\n')"
}

[[ $HOST == "macie" ]] && alias archie='attach_mux archie'
[[ $HOST == "archie" ]] && alias macie='attach_mux macie'

alias s="attach_mux"

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
  local yazi_status
  zle -I
  ycd
  yazi_status=$?
  zle reset-prompt
  return "$yazi_status"
}

zle -N wezterm-open-yazi _wezterm_open_yazi
