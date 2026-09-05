# `t` enters the project workspace through the controller, which owns attachment
# metadata and resets the GUI routing marker when its client detaches.
t() {
  tmux-workspace enter "$@"
}

[[ -o interactive ]] || return 0
autoload -Uz add-zsh-hook add-zle-hook-widget

# A raw `tmux attach` can lose its server before the detached hook runs. Once
# this outer shell owns the terminal again, clear any surviving GUI marker.
# This never claims tmux ownership from shell ancestry.
_tmux_owner_reset() {
  [[ -z $TMUX ]] && printf '\e]1337;SetUserVar=TMUX_WORKSPACE=\a'
  return 0
}
add-zsh-hook -d precmd _tmux_owner_reset
if [[ -z $TMUX ]]; then
  add-zsh-hook precmd _tmux_owner_reset
  return 0
fi

# OSC 133 is interpreted by tmux itself. Do not wrap it in passthrough: the
# outer terminal has a different scrollback. This is independent of WezTerm's
# deliberately disabled shell semantic zones.
_tmux_prompt_start() {
  local command_status=$?
  if [[ ${_tmux_command_running:-0} == 1 ]]; then
    printf '\e]133;D;%d\a' "$command_status"
  fi
  typeset -g _tmux_command_running=0
  printf '\e]133;A\a'
}

_tmux_prompt_end() {
  printf '\e]133;B\a'
}

_tmux_command_start() {
  typeset -g _tmux_command_running=1
  printf '\e]133;C\a'
}

# Reloading shell configuration should not multiply prompt marks.
add-zsh-hook -d precmd _tmux_prompt_start
add-zsh-hook -d preexec _tmux_command_start
add-zle-hook-widget -d line-init _tmux_prompt_end
add-zsh-hook precmd _tmux_prompt_start
add-zsh-hook preexec _tmux_command_start
add-zle-hook-widget line-init _tmux_prompt_end
