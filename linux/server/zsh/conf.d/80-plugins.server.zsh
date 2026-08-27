# zsh-autosuggestions + zsh-syntax-highlighting for headless servers.
#
# Unlike linux/common (which hardcodes Arch paths), a VPS could be Arch,
# Debian/Ubuntu, Fedora, Alpine, ... — each ships these plugins in a different
# prefix, and they may not be installed at all. So we probe a candidate list
# and source the first that exists. Missing plugins are silently skipped
# instead of throwing "no such file" on every prompt.
ZSH_AUTOSUGGEST_STRATEGY=(history completion)

_srv_source_first() {
  local f
  for f in "$@"; do
    if [[ -r "$f" ]]; then
      source "$f"
      return 0
    fi
  done
  return 1
}

# autosuggestions first...
_srv_source_first \
  /usr/share/zsh/plugins/zsh-autosuggestions/zsh-autosuggestions.zsh \
  /usr/share/zsh-autosuggestions/zsh-autosuggestions.zsh \
  /usr/local/share/zsh-autosuggestions/zsh-autosuggestions.zsh \
  "$ZSH/custom/plugins/zsh-autosuggestions/zsh-autosuggestions.zsh"

# ...syntax-highlighting must be sourced last (it wraps the ZLE widgets).
_srv_source_first \
  /usr/share/zsh/plugins/zsh-syntax-highlighting/zsh-syntax-highlighting.zsh \
  /usr/share/zsh-syntax-highlighting/zsh-syntax-highlighting.zsh \
  /usr/local/share/zsh-syntax-highlighting/zsh-syntax-highlighting.zsh \
  "$ZSH/custom/plugins/zsh-syntax-highlighting/zsh-syntax-highlighting.zsh"

unfunction _srv_source_first
