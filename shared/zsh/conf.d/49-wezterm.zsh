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

  local domain from route session to
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

  if [[ -z $WEZTERM_PANE ]]; then
    print -ru2 "mux: not a wezterm pane; $domain"
    return 1
  fi

  local pane
  pane=$(wezterm cli spawn --domain-name "$domain" -- env "HWIRE_SESSION=$session" zsh -l) || return
  wezterm cli activate-pane --pane-id "$pane"
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
