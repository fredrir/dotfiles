# `source` of a file: 126 when an error stopped it, else its status
_zsh_build_status() {
  local code=$?
  (( ${_zsh_build_stopped:-0} )) && code=126
  unset _zsh_build_stopped
  return code
}
