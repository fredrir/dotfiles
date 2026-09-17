[[ -o interactive ]] || return 0

autoload -Uz select-word-style
select-word-style bash

_motion_word_char() {
  local index=$1
  [[ ${BUFFER[index]} == [[:alnum:]_] ]] && return 0
  [[ ${BUFFER[index]} == [.\'] ]] || return 1
  ((index > 1)) || return 1
  [[ ${BUFFER[index-1]} == [[:alnum:]_] && ${BUFFER[index+1]} == [[:alnum:]_] ]]
}

_motion_join_line() {
  [[ $LBUFFER == *$'\n' ]] || return 1
  CUTBUFFER=$'\n'
  LBUFFER=${LBUFFER%$'\n'}
}

_motion_kill_word() {
  local index=$CURSOR
  ((index)) || return 0
  _motion_join_line && return 0
  local head=${LBUFFER%"${LBUFFER##*$'\n'}"}
  local floor=$#head
  if [[ $1 == shell ]]; then
    while ((index > floor)) && [[ ${BUFFER[index]} == [[:space:]] ]]; do ((index--)); done
    while ((index > floor)) && [[ ${BUFFER[index]} != [[:space:]] ]]; do ((index--)); done
    CUTBUFFER=${LBUFFER[index+1,-1]}
    LBUFFER=${LBUFFER[1,index]}
    return 0
  fi
  local gap=$index blanks=0
  while ((gap > floor)) && [[ ${BUFFER[gap]} == [[:space:]] ]]; do ((gap-- , blanks++)); done
  if ((blanks > 1)); then
    index=$gap
  else
    while ((index > floor)) && ! _motion_word_char $index; do ((index--)); done
    while ((index > floor)) && _motion_word_char $index; do ((index--)); done
  fi
  CUTBUFFER=${LBUFFER[index+1,-1]}
  LBUFFER=${LBUFFER[1,index]}
}

_motion_kill_to_line_start() {
  [[ -n $LBUFFER ]] || return 0
  _motion_join_line && return 0
  local head=${LBUFFER%"${LBUFFER##*$'\n'}"}
  CUTBUFFER=${LBUFFER#"$head"}
  LBUFFER=$head
}

_motion_backward_kill_word() { _motion_kill_word word; }
_motion_backward_kill_shell_word() { _motion_kill_word shell; }

zle_highlight=(${zle_highlight[@]:#region:*} region:standout)

_motion_buffer_start() { CURSOR=0; }
_motion_buffer_end() { CURSOR=$#BUFFER; }

_motion_scrollback() {
  if [[ -z $TMUX ]]; then
    printf '\e]1337;SetUserVar=SCROLLBACK=%s\a' "$(print -rn -- "$1" | base64 | tr -d '\r\n')"
  elif [[ $1 == top ]]; then
    command tmux copy-mode \; send-keys -X history-top 2>/dev/null
  else
    command tmux send-keys -X cancel 2>/dev/null
  fi
}

_motion_document_start() {
  if [[ $BUFFER == *$'\n'* ]]; then
    CURSOR=0
  else
    _motion_scrollback top
  fi
}

_motion_document_end() {
  if [[ $BUFFER == *$'\n'* ]]; then
    CURSOR=$#BUFFER
  else
    _motion_scrollback bottom
  fi
}

typeset -gA _MOTION_SELECT=(
  motion-select-backward-char backward-char
  motion-select-forward-char forward-char
  motion-select-backward-word backward-word
  motion-select-forward-word forward-word
  motion-select-beginning-of-line beginning-of-line
  motion-select-end-of-line end-of-line
  motion-select-buffer-start motion-buffer-start
  motion-select-buffer-end motion-buffer-end
)

_motion_select() {
  ((REGION_ACTIVE)) || MARK=$CURSOR
  REGION_ACTIVE=1
  zle -K motion-select
  zle "$_MOTION_SELECT[$WIDGET]"
}

_motion_deselect() {
  REGION_ACTIVE=0
  zle -K main
  zle -U -- "$KEYS"
}

_motion_kill_selection() {
  zle kill-region
  REGION_ACTIVE=0
  zle -K main
}

_motion_replace_selection() {
  zle kill-region
  REGION_ACTIVE=0
  zle -K main
  zle self-insert
}
_insert_newline() {
  LBUFFER+=$'\n'
}

for widget in ${(k)_MOTION_SELECT}; do zle -N $widget _motion_select; done
unset widget

zle -N motion-buffer-start _motion_buffer_start
zle -N motion-buffer-end _motion_buffer_end
zle -N motion-backward-kill-word _motion_backward_kill_word
zle -N motion-backward-kill-shell-word _motion_backward_kill_shell_word
zle -N motion-kill-to-line-start _motion_kill_to_line_start
zle -N motion-deselect _motion_deselect
zle -N motion-kill-selection _motion_kill_selection
zle -N motion-replace-selection _motion_replace_selection
zle -N insert-newline _insert_newline
zle -N motion-document-start _motion_document_start
zle -N motion-document-end _motion_document_end
