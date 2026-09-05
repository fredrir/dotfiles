mux() {
  mux_socket="$HOME/.local/share/wezterm/localmux.sock"
  WEZTERM_UNIX_SOCKET="$mux_socket" wezterm cli --prefer-mux --no-auto-start "$@"
}

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

  local domain from route session to remote_home
  domain=$(mux-route $1) || return

  from=${HOST%%.*}
  to=${domain%-*}
  route=${domain##*-}

  case "$from:$to:$route" in
  macie:archie:cable | macie:archie:wifi | macie:archie:tailscale | \
    archie:macie:cable | archie:macie:wifi | archie:macie:tailscale)
    session="v1:${from}:${to}:${route}:tls"
    ;;
  *)
    print -ru2 "mux: refusing invalid TLS domain metadata: $from -> $domain"
    return 1
    ;;
  esac

  case $to in
  archie) remote_home=/home/fredrir ;;
  macie) remote_home=/Users/fredrir ;;
  esac

  if [[ -z $WEZTERM_PANE ]]; then
    print -ru2 "mux: not a wezterm pane; $domain"
    return 1
  fi

  local -a remote_shell
  remote_shell=(env -i "HOME=$remote_home" "TERM=xterm-256color" "PATH=/usr/local/bin:/usr/bin:/bin" "HWIRE_SESSION=$session" zsh -l)

  wezterm cli spawn --domain-name "$domain" --cwd "$remote_home" -- $remote_shell >/dev/null || return
  wezterm cli kill-pane --pane-id "$WEZTERM_PANE"
}

alias archie='attach_mux archie'
alias macie='attach_mux macie'

alias mtls="~/.config/wezterm/bin/wezterm-mtls"

if [[ -n $WEZTERM_PANE ]]; then
  WEZTERM_SHELL_SKIP_SEMANTIC_ZONES=1
  WEZTERM_SHELL_SKIP_CWD=1
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

# These sequences also arrive through SSH without WEZTERM_PANE in its environment.
[[ -o interactive ]] || return 0

_wezterm_insert_newline() {
  LBUFFER+=$'\n'
}

zle -N wezterm-insert-newline _wezterm_insert_newline
bindkey -M emacs $'\e[13;2u' wezterm-insert-newline
bindkey -M viins $'\e[13;2u' wezterm-insert-newline

_wezterm_open_yazi() {
  local cwd cwd_file yazi_status
  zle -I
  if [[ -n $TMUX ]] && (( $+commands[tmux-workspace] )); then
    cwd_file=$(mktemp -t 'tmux-yazi-cwd.XXXXXX') || return
    {
      tmux-workspace yazi --pane "$TMUX_PANE" --cwd-file "$cwd_file"
      yazi_status=$?
      IFS= read -r -d '' cwd < "$cwd_file"
      [[ -n $cwd && -d $cwd && $cwd != $PWD ]] && builtin cd -- "$cwd"
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
bindkey -M emacs $'\e[115;9u' wezterm-open-yazi
bindkey -M vicmd $'\e[115;9u' wezterm-open-yazi
bindkey -M viins $'\e[115;9u' wezterm-open-yazi
# tmux treats the legacy CSI-u Super modifier as Meta before user-key matching.
# A reserved function-key sequence preserves this widget through every layer.
bindkey -M emacs $'\e[5;30012~' wezterm-open-yazi
bindkey -M vicmd $'\e[5;30012~' wezterm-open-yazi
bindkey -M viins $'\e[5;30012~' wezterm-open-yazi
