autoload -Uz add-zsh-hook
typeset -g _project_completion_signature=''

_load_project_completions() {
  local completion_file=${ZSH_COMPLETION_FILE:-}
  if [[ -z $completion_file ]]; then
    _project_completion_signature=''
    return 0
  fi
  [[ -r $completion_file ]] && (($+functions[compdef])) || return 0

  local contents=$(<"$completion_file")
  local signature="$completion_file:$contents"
  [[ $_project_completion_signature == "$signature" ]] && return 0

  source "$completion_file" || return 0
  _project_completion_signature=$signature
}

add-zsh-hook precmd _load_project_completions
add-zsh-hook chpwd _load_project_completions
