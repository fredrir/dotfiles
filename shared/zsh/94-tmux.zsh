alias t="tmux-workspace"
alias tm="tmux"
alias tml="tmux ls"

tma() {
  tmux attach -t "$@"
}

_tma() {
  local -a sessions
  sessions=(${(f)"$(tmux list-sessions -F '#S' 2>/dev/null)"})
  _describe 'tmux session' sessions
}

compdef _tma tma

_tmux_session_exists() {
  tmux list-sessions -F '#S' 2>/dev/null |
    command grep -Fxq -- "$1"
}

tmk() {
  local name="$1"

  if [[ -z "$name" ]]; then
    echo "usage: tmk <session>"
    return 1
  fi

  if ! _tmux_session_exists "$name"; then
    echo "tmk: no such session: $name"
    return 1
  fi

  tmux kill-session -t "$name"
}

_tmk() {
  local -a sessions
  sessions=(${(f)"$(tmux list-sessions -F '#S' 2>/dev/null)"})
  _describe 'tmux session' sessions
}

compdef _tmk tmk

tmn() {
  local name="$1"

  if [[ -z "$name" ]]; then
    local i=1

    while _tmux_session_exists "sesh-$i"; do
      ((i++))
    done

    name="sesh-$i"
  fi

  if _tmux_session_exists "$name"; then
    # Existing session
    if [[ -n "$TMUX" ]]; then
      tmux switch-client -t "$name"
    else
      tmux attach -t "$name"
    fi
  else
    # New session
    if [[ -n "$TMUX" ]]; then
      tmux new-session -d -s "$name"
      tmux switch-client -t "$name"
    else
      tmux new-session -s "$name"
    fi
  fi
}

_tmn() {
  local -a sessions
  sessions=(${(f)"$(tmux list-sessions -F '#S' 2>/dev/null)"})
  _describe 'tmux session' sessions
}

compdef _tmn tmn

tmc() {
  local name="codex-agents"

  if _tmux_session_exists "$name"; then
    return 0
  fi

  tmux new-session -d -s "$name" 'exec codex agents'
}

tmc

# Open Vscode Fix
_tmux_report_cwd() {
  (($+commands[wezterm] || $+functions[wezterm])) || return 0
  wezterm set-working-directory --tmux-passthru enable "$PWD" "${WEZTERM_HOSTNAME:-${HOST%%.*}}" 2>/dev/null
  return 0
}
