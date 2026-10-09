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

  if [[ -o interactive && $TERM != (linux|dumb) && ${WEZTERM_SHELL_SKIP_ALL-} != 1 && -z ${WEZTERM_SHELL_SKIP_USER_VARS-} ]]; then
    _wezterm_user_var() {
      _b64 "$2"
      if [[ -z ${TMUX-} ]]; then
        printf -v REPLY '\e]1337;SetUserVar=%s=%s\a' "$1" "$REPLY"
      else
        printf -v REPLY '\ePtmux;\e\e]1337;SetUserVar=%s=%s\a\e\\' "$1" "$REPLY"
      fi
    }
    () {
      local REPLY name value
      typeset -g _wezterm_prompt_vars=
      for name value in WEZTERM_PROG '' WEZTERM_USER "$USERNAME" WEZTERM_IN_TMUX "${${TMUX:+1}:-0}" WEZTERM_HOST "$WEZTERM_HOSTNAME"; do
        _wezterm_user_var $name "$value"
        _wezterm_prompt_vars+=$REPLY
      done
    }
    __wezterm_user_vars_precmd() {
      print -rn -- "$_wezterm_prompt_vars"
    }
    __wezterm_user_vars_preexec() {
      local REPLY
      _wezterm_user_var WEZTERM_PROG "$1"
      print -rn -- "$REPLY"
    }
    precmd_functions+=(__wezterm_user_vars_precmd)
    preexec_functions+=(__wezterm_user_vars_preexec)
  fi
fi

if (($+functions[omz_termsupport_cwd])); then
  _omz_urlencode_path() {
    emulate -L zsh -o no_multibyte
    local in=$1 out='' byte
    local -i i
    for ((i = 1; i <= ${#in}; i++)); do
      byte=${in[i]}
      if [[ $byte == [A-Za-z0-9\;/?:@\&=+'$',_.\!\~\*\(\)-] ]]; then
        out+=$byte
      else
        out+=%$(([##16] #byte))
      fi
    done
    REPLY=$out
  }
  omz_termsupport_cwd() {
    setopt localoptions unset
    local REPLY URL_HOST URL_PATH
    if [[ ${langinfo[CODESET]} == (UTF-8|utf8|US-ASCII) ]]; then
      if [[ $_omz_cwd_key != "$HOST/$PWD" ]]; then
        _omz_urlencode_path $HOST
        _omz_cwd_host=$REPLY
        _omz_urlencode_path $PWD
        _omz_cwd_path=$REPLY
        _omz_cwd_key="$HOST/$PWD"
      fi
      URL_HOST=$_omz_cwd_host URL_PATH=$_omz_cwd_path
    else
      URL_HOST="$(omz_urlencode -P $HOST)" || return 1
      URL_PATH="$(omz_urlencode -P $PWD)" || return 1
    fi
    [[ -z "$KONSOLE_PROFILE_NAME" && -z "$KONSOLE_DBUS_SESSION" ]] || URL_HOST=""
    printf "\e]7;file://%s%s\e\\" "${URL_HOST}" "${URL_PATH}"
  }
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
