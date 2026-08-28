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

  local domain
  domain=$(mux-route $1) || return

  if [[ -z $WEZTERM_PANE ]]; then
    print -ru2 "mux: not a wezterm pane, so there is nowhere to put $domain"
    return 1
  fi

  # The peer's shell takes this pane's place. It cannot land in this very tab:
  # wezterm will not move a live pane to another domain, and a cross-domain
  # split is asked of the peer, which has never heard of a local pane id. So
  # it is spawned at the end of the tab bar and this pane is closed behind it.
  # The attach chord reaches this by typing it -- see keymap/init.lua.
  local pane
  pane=$(wezterm cli spawn --domain-name "$domain") || return
  wezterm cli activate-pane --pane-id "$pane"
  wezterm cli kill-pane --pane-id "$WEZTERM_PANE"
}

alias archie='mux archie'
alias macie='mux macie'
