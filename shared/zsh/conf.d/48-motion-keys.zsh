[[ -o interactive ]] || return 0

autoload -Uz select-word-style
select-word-style bash

# AppKit joins `.` and `'` into a word when both neighbours are word characters,
# which is what keeps foo.bar.baz and don't single words. See UAX #29 WB6/WB7.
_motion_word_char() {
  local index=$1
  [[ ${BUFFER[index]} == [[:alnum:]_] ]] && return 0
  [[ ${BUFFER[index]} == [.\'] ]] || return 1
  ((index > 1)) || return 1
  [[ ${BUFFER[index - 1]} == [[:alnum:]_] && ${BUFFER[index + 1]} == [[:alnum:]_] ]]
}

# On an empty line both kills remove just the break, so neither ever gets stuck
# and both walk back up the buffer. AppKit leaves the cursor sitting there.
_motion_join_line() {
  [[ $LBUFFER == *$'\n' ]] || return 1
  CUTBUFFER=$'\n'
  LBUFFER=${LBUFFER%$'\n'}
}

# Follows AppKit deleteWordBackward: within a line — skip every non-word
# character, then remove one run of word characters — with two deliberate
# deviations: a line break is a floor rather than something to run through, and
# a gap of two or more blanks is its own press. A single trailing blank still
# rides along with the word it follows, which is what AppKit does, so only a
# deliberate gap survives a press. `shell` keeps readline's unix-word-rubout.
_motion_kill_word() {
  local index=$CURSOR
  ((index)) || return 0
  _motion_join_line && return 0
  local head=${LBUFFER%"${LBUFFER##*$'\n'}"}
  local floor=$#head
  if [[ $1 == shell ]]; then
    while ((index > floor)) && [[ ${BUFFER[index]} == [[:space:]] ]]; do ((index--)); done
    while ((index > floor)) && [[ ${BUFFER[index]} != [[:space:]] ]]; do ((index--)); done
    CUTBUFFER=${LBUFFER[index + 1, -1]}  # shuck: ignore=C001 # CUTBUFFER is the ZLE kill ring
    LBUFFER=${LBUFFER[1, index]}
    return 0
  fi
  local gap=$index blanks=0
  while ((gap > floor)) && [[ ${BUFFER[gap]} == [[:space:]] ]]; do ((gap--, blanks++)); done
  if ((blanks > 1)); then
    index=$gap
  else
    while ((index > floor)) && ! _motion_word_char $index; do ((index--)); done
    while ((index > floor)) && _motion_word_char $index; do ((index--)); done
  fi
  CUTBUFFER=${LBUFFER[index + 1, -1]}  # shuck: ignore=C001 # CUTBUFFER is the ZLE kill ring
  LBUFFER=${LBUFFER[1, index]}
}

# Follows AppKit deleteToBeginningOfLine: except on an empty line, where AppKit
# stalls and this joins the lines so repeated presses keep clearing upwards.
_motion_kill_to_line_start() {
  [[ -n $LBUFFER ]] || return 0
  _motion_join_line && return 0
  local head=${LBUFFER%"${LBUFFER##*$'\n'}"}
  CUTBUFFER=${LBUFFER#"$head"}  # shuck: ignore=C001 # CUTBUFFER is the ZLE kill ring
  LBUFFER=$head
}

_motion_backward_kill_word() { _motion_kill_word word }
_motion_backward_kill_shell_word() { _motion_kill_word shell }

zle -N motion-backward-kill-word _motion_backward_kill_word
zle -N motion-backward-kill-shell-word _motion_backward_kill_shell_word
zle -N motion-kill-to-line-start _motion_kill_to_line_start

# zsh binds Ctrl-U to kill-whole-line; macOS Cmd+Backspace only clears leftwards.
for keymap in emacs viins; do
  bindkey -M $keymap '^W' motion-backward-kill-shell-word
  bindkey -M $keymap '^U' motion-kill-to-line-start
  bindkey -M $keymap '^[^?' motion-backward-kill-word
done
unset keymap

zle_highlight=(${zle_highlight[@]:#region:*} region:standout)

_motion_buffer_start() { CURSOR=0 }
_motion_buffer_end() { CURSOR=$#BUFFER }

zle -N motion-buffer-start _motion_buffer_start
zle -N motion-buffer-end _motion_buffer_end

_motion_scrollback() {
  if [[ -z $TMUX ]]; then
    printf '\e]1337;SetUserVar=SCROLLBACK=%s\a' "$(print -rn -- "$1" | base64 | tr -d '\r\n')"
  elif [[ $1 == top ]]; then
    command tmux copy-mode \; send-keys -X history-top 2>/dev/null
  else
    command tmux send-keys -X cancel 2>/dev/null
  fi
}

# Cmd+Up/Down reads as a document motion while a command spans several lines,
# and as a scrollback jump while the buffer is the single line it usually is.
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

zle -N motion-document-start _motion_document_start
zle -N motion-document-end _motion_document_end

bindkey -N motion-select emacs

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

# The anchor is dropped only when the region is inactive, so repeated presses
# grow one selection instead of restarting it at the current cursor.
_motion_select() {
  ((REGION_ACTIVE)) || MARK=$CURSOR  # shuck: ignore=C001 # MARK is read by ZLE
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

for widget in ${(k)_MOTION_SELECT}; do zle -N $widget _motion_select; done
unset widget

zle -N motion-deselect _motion_deselect
zle -N motion-kill-selection _motion_kill_selection
zle -N motion-replace-selection _motion_replace_selection

# Anything unclaimed collapses the region and replays through the main keymap,
# so leaving a selection never needs a dedicated escape key.
bindkey -M motion-select -R '^@'-'\M-^?' motion-deselect
bindkey -M motion-select -R ' '-'~' motion-replace-selection
bindkey -M motion-select '^?' motion-kill-selection
bindkey -M motion-select '^H' motion-kill-selection
bindkey -M motion-select $'\e[3~' motion-kill-selection

# Cmd+Up/Down stay out of motion-select so they collapse a region like macOS.
for keymap in emacs viins; do
  bindkey -M $keymap $'\e[1;5H' motion-document-start
  bindkey -M $keymap $'\e[1;5F' motion-document-end
done

for keymap in emacs viins motion-select; do
  bindkey -M $keymap $'\e[1;2D' motion-select-backward-char
  bindkey -M $keymap $'\e[1;2C' motion-select-forward-char
  bindkey -M $keymap $'\e[1;2A' motion-select-backward-char
  bindkey -M $keymap $'\e[1;2B' motion-select-forward-char
  bindkey -M $keymap $'\e[1;4D' motion-select-backward-word
  bindkey -M $keymap $'\e[1;4C' motion-select-forward-word
  bindkey -M $keymap $'\e[1;2H' motion-select-beginning-of-line
  bindkey -M $keymap $'\e[1;2F' motion-select-end-of-line
  bindkey -M $keymap $'\e[1;6H' motion-select-buffer-start
  bindkey -M $keymap $'\e[1;6F' motion-select-buffer-end
done
unset keymap
