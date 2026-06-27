# Strip trailing newlines from pasted text.
#
# zsh uses "bracketed paste" so a paste is never auto-executed (good safety
# feature). The downside: if the copied text ends in a newline, that newline
# is inserted literally, leaving the cursor on an extra blank line below the
# paste. oh-my-zsh binds the paste widget to bracketed-paste-magic, which
# exposes a paste-finish hook — use it to trim trailing CR/LF so a paste that
# included a line ending doesn't leave a blank line. Interior newlines (real
# multi-line pastes) are preserved.
_strip_paste_trailing_newline() {
  emulate -L zsh
  setopt extendedglob
  PASTED=${PASTED%%[$'\r\n']##}
}
zstyle :bracketed-paste-magic paste-finish _strip_paste_trailing_newline
