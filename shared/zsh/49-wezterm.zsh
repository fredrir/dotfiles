mux() {
  local mux_socket="$HOME/.local/share/wezterm/localmux.sock"
  WEZTERM_UNIX_SOCKET="$mux_socket" wezterm cli --prefer-mux --no-auto-start "$@"
}

# TODO: Disallow connection when already connected
attach_mux() {
  emulate -L zsh

  if [[ $1 == (-l|--list) ]]; then
    mux-route --list $2
    return
  fi

  local request_name=ATTACH_MUX
  if [[ $1 == (-a|--adopt) ]]; then
    request_name=ADOPT_MUX
    shift
  fi

  if [[ -z $WEZTERM_PANE ]]; then
    print -ru2 'attach_mux: not a wezterm pane'
    return 1
  fi
  local target=${1:-peer}
  if [[ $target == *[^[:alnum:]._-]* ]]; then
    print -ru2 "attach_mux: invalid host: $target"
    return 1
  fi

  zmodload zsh/datetime
  local id="$$:$EPOCHREALTIME"
  local tty_state reply
  tty_state=$(stty -g </dev/tty) || return 1
  stty -echo -icanon </dev/tty
  {
    if [[ $request_name == ATTACH_MUX ]]; then
      set_user_var $request_name "v2:$target:$id"$'\0'"$HOME"$'\0'"$PWD"
    else
      set_user_var $request_name "v1:$target:$id"
    fi
    while IFS= read -r -t 60 -d $'\a' reply </dev/tty; do
      [[ $reply == "$id "* ]] || continue
      reply=${reply#"$id "}
      [[ -z $reply ]] && return 0
      print -ru2 -- "$reply"
      return 1
    done
    print -ru2 'attach_mux: no reply from wezterm'
    return 1
  } always {
    stty "$tty_state" </dev/tty
  }
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

if [[ $WEZTERM_PANE_READY == 1 && -n $WEZTERM_PANE ]]; then
  unset WEZTERM_PANE_READY
  autoload -Uz add-zle-hook-widget
  _wezterm_pane_ready() {
    zle -R
    printf '\e]1337;SetUserVar=WEZTERM_PANE_READY=MQ==\a'
    add-zle-hook-widget -d line-init _wezterm_pane_ready
  }
  zle -N _wezterm_pane_ready
  add-zle-hook-widget line-init _wezterm_pane_ready
fi

_wezterm_open_yazi() {
  local yazi_status
  zle -I
  ycd
  yazi_status=$?
  zle reset-prompt
  return "$yazi_status"
}

zle -N wezterm-open-yazi _wezterm_open_yazi

_wezterm_cd_preview() {
  # Preview the destination's venv without changing the environment ahead of direnv.
  local REPLY
  if (($+functions[_find_python_project_venv])); then
    _find_python_project_venv
    local -x VIRTUAL_ENV=$REPLY
    [[ -n $VIRTUAL_ENV ]] || unset VIRTUAL_ENV
  fi

  zle reset-prompt
  zle -R
  return 0
}

_wezterm_cd() {
  local destination=${1:-}
  if [[ -z $destination || ! -d $destination ]]; then
    zle -M "Not an existing directory: ${destination:-<unset>}"
    return 1
  fi

  [[ ${PWD:A} == "${destination:A}" ]] && return 0

  # Paint before slow chpwd hooks; keep their order and finish them before accepting input.
  local -a chpwd_functions=(_wezterm_cd_preview "${chpwd_functions[@]}")
  builtin cd -- "$destination" || return
  zle reset-prompt
}

_wezterm_cd_pyparser() { _wezterm_cd "$PYPARSER"; }
_wezterm_cd_dotfiles() { _wezterm_cd "$DOTFILES"; }

zle -N wezterm-cd-pyparser _wezterm_cd_pyparser
zle -N wezterm-cd-dotfiles _wezterm_cd_dotfiles

wez-restart() {

  if [[ -n $MACOS ]]; then
    launchctl kickstart -k gui/$(id -u)/com.fredrir.wezterm-mux || true
    open /Applications/WezTerm.app
    exit 0
  fi

  if [[ -n $LINUX ]]; then
    sudo pkill wezterm-mux-ser
    exit 0
  fi
}
