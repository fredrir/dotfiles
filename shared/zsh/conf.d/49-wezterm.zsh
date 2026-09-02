_wezterm_gui_socket() {
  emulate -L zsh

  local dir=${WEZTERM_GUI_SOCKET_DIR:-$HOME/.local/share/wezterm}
  local socket pid
  local -a alive

  for socket in "$dir"/gui-sock-*(N); do
    [[ -S $socket ]] || continue
    pid=${socket##*gui-sock-}
    [[ $pid == <-> ]] || continue
    kill -0 "$pid" 2>/dev/null && alive+=("$socket")
  done

  if (( ${#alive} == 1 )); then
    print -r -- "${alive[1]}"
    return 0
  fi

  local default=$dir/default-org.wezfurlong.wezterm
  if [[ -S $default ]]; then
    pid=${default:A:t}
    pid=${pid##*gui-sock-}
    if [[ $pid == <-> ]] && kill -0 "$pid" 2>/dev/null; then
      print -r -- "$default"
      return 0
    fi
  fi

  return 1
}

mux() {
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

  local domain from route session to home
  domain=$(mux-route $1) || return

  from=${HOST%%.*}
  to=${domain%-*}
  route=${domain##*-}

  case "$from:$to:$route" in
    macie:archie:cable|macie:archie:wifi|macie:archie:tailscale|\
    archie:macie:cable|archie:macie:wifi|archie:macie:tailscale)
      session="v1:${from}:${to}:${route}:tls"
      ;;
    *)
      print -ru2 "mux: refusing invalid TLS domain metadata: $from -> $domain"
      return 1
      ;;
  esac

  case $to in
    archie) home=/home/fredrir ;;
    macie) home=/Users/fredrir ;;
  esac

  if [[ -z $WEZTERM_PANE ]]; then
    print -ru2 "mux: not a wezterm pane; $domain"
    return 1
  fi

  local gui pane
  if gui=$(_wezterm_gui_socket); then
    pane=$(WEZTERM_UNIX_SOCKET=$gui wezterm cli spawn --domain-name "$domain" --cwd "$home" -- env "HWIRE_SESSION=$session" zsh -l) || return
    WEZTERM_UNIX_SOCKET=$gui wezterm cli activate-pane --pane-id "$pane"
  else
    print -ru2 "mux: no live wezterm gui; attaching through the local mux server (two hops)"
    pane=$(wezterm cli spawn --domain-name "$domain" --cwd "$home" -- env "HWIRE_SESSION=$session" zsh -l) || return
    wezterm cli activate-pane --pane-id "$pane"
  fi

  wezterm cli kill-pane --pane-id "$WEZTERM_PANE"
}

alias archie='mux archie'
alias macie='mux macie'

alias mtls="~/.config/wezterm/bin/wezterm-mtls"

[[ -n $WEZTERM_PANE ]] || return 0

WEZTERM_SHELL_SKIP_SEMANTIC_ZONES=1
WEZTERM_SHELL_SKIP_CWD=1
: ${WEZTERM_HOSTNAME:=$HOST}

for _wezterm_sh in \
  /Applications/WezTerm.app/Contents/Resources/wezterm.sh \
  /etc/profile.d/wezterm.sh \
  /usr/share/wezterm/shell-integration/wezterm.sh
do
  [[ -r $_wezterm_sh ]] || continue
  source "$_wezterm_sh"
  break
done
unset _wezterm_sh

_wezterm_insert_newline() {
  LBUFFER+=$'\n'
}

zle -N wezterm-insert-newline _wezterm_insert_newline
bindkey -M emacs $'\e[13;2u' wezterm-insert-newline
bindkey -M viins $'\e[13;2u' wezterm-insert-newline
