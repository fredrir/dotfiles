# Run from the repository root: zsh -dfi shared/zsh/tests/motion-keys.zsh
source shared/zsh/conf.d/48-motion-keys.zsh  # shuck: ignore=C003 # test harness sources by repository-root path

# Cmd+Backspace must clear leftwards only; zsh defaults to kill-whole-line.
[[ $(bindkey -M emacs '^U') == *motion-kill-to-line-start ]] || exit 1
[[ $(bindkey -M viins '^U') == *motion-kill-to-line-start ]] || exit 1
[[ $(bindkey -M emacs '^W') == *motion-backward-kill-shell-word ]] || exit 1
[[ $(bindkey -M emacs '^[^?') == *motion-backward-kill-word ]] || exit 1

typeset -a zle_calls
zle() { zle_calls+=("$1"); return 0 }

# Five presses per case, so a fix to one step cannot regress the step after it.
kills() {
  local buffer=$1 widget=$2
  local -a result=()
  repeat 5; do
    LBUFFER=$buffer; RBUFFER=''; BUFFER=$buffer; CURSOR=$#buffer
    $widget
    buffer=$LBUFFER
    result+=("${buffer//$'\n'/\\n}")
  done
  print -r -- "${(j:|:)result}"
}

# Three deliberate deviations from AppKit, each listed here so a behaviour
# change can never pass as an AppKit match by accident:
#   1. a line break is a floor, not something to delete through;
#   2. on an empty line both kills remove the break instead of stalling;
#   3. a gap of two or more blanks is its own press, while a single blank still
#      rides along with the word it follows the way AppKit does.
typeset -A word_deviations=(
  '123\n45\n   '    '123\n45\n|123\n45|123\n|123|'
  '123\n45\n'       '123\n45|123\n|123||'
  '123\n45'          '123\n|123|||'
  'one two\nthree'   'one two\n|one two|one ||'
  'a  b'             'a  |a|||'
  'trailing;;;   '   'trailing;;;||||'
  '123 456  '        '123 456|123 |||'
  '123 456   '       '123 456|123 |||'
  $'123\t\t456'      $'123\t\t|123|||'
  'a b  c'           'a b  |a b|a ||'
)
typeset -A line_deviations=(
  '123\n45\n   '    '123\n45\n|123\n45|123\n|123|'
  '123\n45\n'       '123\n45|123\n|123||'
  '123\n45'          '123\n|123|||'
  'one two\nthree'   'one two\n|one two|||'
)

# Ground truth is read out of AppKit rather than described second-hand; see
# support/appkit-motion.swift for the generator that produced this table.
typeset -g exact=0 deviating=0
appkit() {
  local line section='' input expected
  local newline=$'\n'
  local -a args
  while IFS= read -r line; do
    [[ -z $line ]] && continue
    if [[ $line == '#'* ]]; then section=${line#\# }; continue; fi
    eval "args=($line)"
    input=${args[1]//\\n/$newline}
    case $section in
      (deleteWordBackward:)
        expected=${word_deviations[${args[1]}]:-${args[2]}}
        [[ $(kills "$input" _motion_backward_kill_word) == "$expected" ]] || return 1 ;;
      (deleteToBeginningOfLine:)
        expected=${line_deviations[${args[1]}]:-${args[2]}}
        [[ $(kills "$input" _motion_kill_to_line_start) == "$expected" ]] || return 1 ;;
      (*) return 1 ;;
    esac
    if [[ $expected == "${args[2]}" ]]; then ((exact++)); else ((deviating++)); fi
  done
  return 0
}
appkit < shared/zsh/tests/support/appkit-motion.txt || exit 1

# A stale deviation would otherwise sit unnoticed once behaviour caught back up.
((exact == 30 && deviating == 14)) || exit 1

# A lone blank is absorbed by the word; a deliberate gap survives one press.
[[ $(kills '123 456 ' _motion_backward_kill_word) == '123 ||||' ]] || exit 1
[[ $(kills '123 456  ' _motion_backward_kill_word) == '123 456|123 |||' ]] || exit 1

# Cmd+Backspace must keep clearing upwards instead of stalling on a blank line.
[[ $(kills $'123 45\n12\n' _motion_kill_to_line_start) == '123 45\n12|123 45\n|123 45||' ]] || exit 1

# Ctrl-W has no AppKit counterpart; it keeps readline's unix-word-rubout.
[[ $(kills '123 45 67 ' _motion_backward_kill_shell_word) == '123 45 |123 |||' ]] || exit 1
[[ $(kills '/opt/dotfiles/shared/wezterm' _motion_backward_kill_shell_word) == '||||' ]] || exit 1

# Whatever was removed still reaches the kill ring, so Ctrl-Y can put it back.
LBUFFER=$'ab cd'; RBUFFER=''; BUFFER=$LBUFFER; CURSOR=5; CUTBUFFER=''
_motion_backward_kill_word
[[ $CUTBUFFER == cd && $LBUFFER == 'ab ' ]] || exit 1
LBUFFER=$'ab\ncd'; RBUFFER=''; BUFFER=$LBUFFER; CURSOR=5; CUTBUFFER=''
_motion_kill_to_line_start
[[ $CUTBUFFER == cd && $LBUFFER == $'ab\n' ]] || exit 1
print -r -- 'word granularity: passed'

typeset -A expected=(
  $'\e[1;2D' motion-select-backward-char
  $'\e[1;2C' motion-select-forward-char
  $'\e[1;4D' motion-select-backward-word
  $'\e[1;4C' motion-select-forward-word
  $'\e[1;2H' motion-select-beginning-of-line
  $'\e[1;2F' motion-select-end-of-line
  $'\e[1;6H' motion-select-buffer-start
  $'\e[1;6F' motion-select-buffer-end
)
for sequence widget in ${(kv)expected}; do
  [[ $(bindkey -M emacs "$sequence") == *" $widget" ]] || exit 1
  [[ $(bindkey -M motion-select "$sequence") == *" $widget" ]] || exit 1
done
[[ $(bindkey -M emacs $'\e[1;5H') == *motion-document-start ]] || exit 1
[[ $(bindkey -M emacs $'\e[1;5F') == *motion-document-end ]] || exit 1
[[ $(bindkey -M motion-select '^?') == *motion-kill-selection ]] || exit 1
[[ $(bindkey -M motion-select 'a') == *motion-replace-selection ]] || exit 1
[[ $(bindkey -M motion-select '^X') == *motion-deselect ]] || exit 1
[[ $(bindkey -M motion-select $'\e[1;5H') != *motion-document-start ]] || exit 1
print -r -- 'motion bindings: passed'

# The anchor is set once, so a second press grows the same region.
MARK=0 CURSOR=4 REGION_ACTIVE=0 WIDGET=motion-select-forward-char zle_calls=()
_motion_select
((MARK == 4 && REGION_ACTIVE == 1)) || exit 1
[[ ${zle_calls[-1]} == forward-char ]] || exit 1
CURSOR=7 WIDGET=motion-select-end-of-line
_motion_select
((MARK == 4)) || exit 1
[[ ${zle_calls[-1]} == end-of-line ]] || exit 1

zle_calls=()
_motion_kill_selection
((REGION_ACTIVE == 0)) || exit 1
[[ ${zle_calls[1]} == kill-region && ${zle_calls[2]} == -K ]] || exit 1

zle_calls=()
_motion_replace_selection
[[ ${zle_calls[1]} == kill-region && ${zle_calls[-1]} == self-insert ]] || exit 1
print -r -- 'selection region: passed'

typeset -a scrollback_calls
_motion_scrollback() { scrollback_calls+=("$1") }

# A single-line buffer has no document to traverse, so the scrollback moves.
BUFFER='ls -la' CURSOR=3 scrollback_calls=()
_motion_document_start
[[ ${scrollback_calls[1]} == top && $CURSOR == 3 ]] || exit 1
_motion_document_end
[[ ${scrollback_calls[2]} == bottom && $CURSOR == 3 ]] || exit 1

BUFFER=$'for file in *\ndo\n  print $file\ndone' CURSOR=8 scrollback_calls=()
_motion_document_start
((CURSOR == 0 && $#scrollback_calls == 0)) || exit 1
_motion_document_end
((CURSOR == $#BUFFER && $#scrollback_calls == 0)) || exit 1
print -r -- 'document motions: passed'

unfunction _motion_scrollback
source shared/zsh/conf.d/48-motion-keys.zsh  # shuck: ignore=C003 # test harness sources by repository-root path

# Outside tmux the pane has no scrollback client, so WezTerm is asked directly.
unset TMUX
request=$(_motion_scrollback top)
[[ $request == $'\e]1337;SetUserVar=SCROLLBACK='*$'\a' ]] || exit 1
encoded=${request#$'\e]1337;SetUserVar=SCROLLBACK='}
[[ $(print -rn -- ${encoded%$'\a'} | base64 -d) == top ]] || exit 1

typeset -gx TMUX=/tmp/isolated-test,123,1
stub_dir=$(mktemp -d) || exit 1
cat > "$stub_dir/tmux" <<'STUB'
#!/bin/sh
printf '%s ' "$@"
STUB
chmod +x "$stub_dir/tmux"
path=("$stub_dir" $path)
hash -r
[[ $(_motion_scrollback top) == 'copy-mode ; send-keys -X history-top ' ]] || exit 1
[[ $(_motion_scrollback bottom) == 'send-keys -X cancel ' ]] || exit 1
command rm -rf -- "$stub_dir"
print -r -- 'scrollback handoff: passed'
