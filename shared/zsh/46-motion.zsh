[[ -o interactive ]] || return 0

autoload -Uz select-word-style add-zle-hook-widget
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

if [[ -n $THEME_SELECTION_FG && -n $THEME_SELECTION_BG ]]; then
  zle_highlight=(${zle_highlight[@]:#region:*} "region:fg=$THEME_SELECTION_FG,bg=$THEME_SELECTION_BG")
else
  zle_highlight=(${zle_highlight[@]:#region:*} region:standout)
fi

_motion_buffer_start() { CURSOR=0; }
_motion_buffer_end() { CURSOR=$#BUFFER; }
_motion_up_line() { zle up-line || CURSOR=0; }
_motion_down_line() { zle down-line || CURSOR=$#BUFFER; }

_motion_scrollback() {
  printf '\e]1337;SetUserVar=SCROLLBACK=%s\a' "$(print -rn -- "$1" | base64 | tr -d '\r\n')"
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

typeset -gA _MOTION_REGION_PAYLOAD=([0]=MA== [1]=MQ==)
typeset -g _MOTION_REGION_STATE=

_motion_announce_region() {
  [[ $1 == $_MOTION_REGION_STATE ]] && return 0
  _MOTION_REGION_STATE=$1
  [[ -n $WEZTERM_PANE ]] || return 0
  printf '\e]1337;SetUserVar=ZLE_SELECTION=%s\a' "$_MOTION_REGION_PAYLOAD[$1]"
}

_motion_clipboard_put() {
  if [[ -n $WEZTERM_PANE ]]; then
    printf '\e]52;c;%s\a' "$(print -rn -- "$1" | base64 | tr -d '\r\n')"
    return 0
  fi
  local -a copier
  if has_cmd pbcopy; then
    copier=(pbcopy)
  elif has_cmd wl-copy; then
    copier=(wl-copy --type text/plain)
  elif has_cmd xclip; then
    copier=(xclip -selection clipboard)
  else
    return 1
  fi
  print -rn -- "$1" | "${copier[@]}"
}

typeset -gA _MOTION_SELECT=(
  motion-select-backward-char backward-char
  motion-select-forward-char forward-char
  motion-select-backward-word backward-word
  motion-select-forward-word forward-word
  motion-select-up-line motion-up-line
  motion-select-down-line motion-down-line
  motion-select-beginning-of-line beginning-of-line
  motion-select-end-of-line end-of-line
  motion-select-buffer-start motion-buffer-start
  motion-select-buffer-end motion-buffer-end
)

_motion_select() {
  ((REGION_ACTIVE)) || MARK=$CURSOR
  REGION_ACTIVE=1
  zle -K motion-select
  _motion_announce_region 1
  zle "$_MOTION_SELECT[$WIDGET]"
}

_motion_end_region() {
  REGION_ACTIVE=0
  zle -K main
  _motion_announce_region 0
}

_motion_deselect() {
  _motion_end_region
  zle -U -- "$KEYS"
}

_motion_cut_region() {
  local start=$((MARK < CURSOR ? MARK : CURSOR))
  local end=$((MARK > CURSOR ? MARK : CURSOR))
  BUFFER=${BUFFER[1,start]}${BUFFER[end+1,-1]}
  CURSOR=$start
}

_motion_replace_selection() {
  _motion_cut_region
  _motion_end_region
  zle -U -- "$KEYS"
}

_motion_kill_selection() {
  zle kill-region
  _motion_end_region
}

_motion_copy_selection() {
  ((REGION_ACTIVE)) || return 0
  zle copy-region-as-kill
  _motion_clipboard_put "$CUTBUFFER"
}

_insert_newline() {
  LBUFFER+=$'\n'
}

_motion_region_reset() { _motion_announce_region 0; }

for widget in ${(k)_MOTION_SELECT}; do zle -N $widget _motion_select; done
unset widget

zle -N motion-buffer-start _motion_buffer_start
zle -N motion-buffer-end _motion_buffer_end
zle -N motion-up-line _motion_up_line
zle -N motion-down-line _motion_down_line
zle -N motion-backward-kill-word _motion_backward_kill_word
zle -N motion-backward-kill-shell-word _motion_backward_kill_shell_word
zle -N motion-kill-to-line-start _motion_kill_to_line_start
zle -N motion-deselect _motion_deselect
zle -N motion-kill-selection _motion_kill_selection
zle -N motion-replace-selection _motion_replace_selection
zle -N motion-copy-selection _motion_copy_selection
zle -N insert-newline _insert_newline
zle -N motion-document-start _motion_document_start
zle -N motion-document-end _motion_document_end

add-zle-hook-widget zle-line-init _motion_region_reset
