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
    print -ru2 "mux: not a wezterm pane; $domain"
    return 1
  fi

  local pane
  pane=$(wezterm cli spawn --domain-name "$domain") || return
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
