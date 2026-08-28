MUX_SOCKET="$HOME/.local/share/wezterm/localmux.sock"

mux() {
  emulate -L zsh

  local list=0
  if [[ $1 == -l || $1 == --list ]]; then
    list=1
    shift
  fi

  if [[ -n $1 && $1 == "$(uname -n)" ]]; then
    print -ru2 "mux: $1 is this machine; its panes are already in localmux"
    return 2
  fi

  if ((list)); then
    mux-route --list $1
    return
  fi

  local domain
  domain=$(mux-route $1) || return

  WEZTERM_UNIX_SOCKET="$MUX_SOCKET" \
    wezterm cli spawn --domain-name "$domain" --new-window >/dev/null || return

  print -r -- "attached $domain"
}

alias archie='mux archie'
alias macie='mux macie'
