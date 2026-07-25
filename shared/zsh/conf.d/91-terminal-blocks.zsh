[[ -o interactive ]] || return 0
[[ -n ${KONSOLE_DBUS_SESSION:-} ]] || return 0
(( $+commands[python3] && $+commands[wl-copy] && $+commands[wl-paste] )) || return 0

_terminal_blocks_copy() {
  emulate -L zsh
  setopt pipefail

  local requested="$1"
  local clear_input=0
  local parser="${XDG_CONFIG_HOME:-$HOME/.config}/terminal_blocks/cli.py"

  if [[ $requested == current ]]; then
    case "$BUFFER" in
      <1->)
        requested="$BUFFER"
        clear_input=1
        ;;
      a|all)
        requested=all
        clear_input=1
        ;;
      *)
        requested=1
        ;;
    esac
  fi

  if command wl-paste --primary --no-newline 2>/dev/null \
    | command python3 "$parser" "$requested" 2>/dev/null \
    | command wl-copy 2>/dev/null; then
    if (( clear_input )); then
      BUFFER=""
      CURSOR=0
    fi
    zle -M "Copied terminal blocks"
  else
    zle -M "No complete terminal blocks found"
  fi
}

_terminal_blocks_copy_current() {
  _terminal_blocks_copy current
}

_terminal_blocks_copy_all() {
  _terminal_blocks_copy all
}

zle -N terminal-blocks-copy _terminal_blocks_copy_current
zle -N terminal-blocks-copy-all _terminal_blocks_copy_all

for _terminal_blocks_keymap in emacs viins vicmd; do
  bindkey -M "$_terminal_blocks_keymap" '^[[99~' terminal-blocks-copy
  bindkey -M "$_terminal_blocks_keymap" '^[[100~' terminal-blocks-copy-all
done

unset _terminal_blocks_keymap
