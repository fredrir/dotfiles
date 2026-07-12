(( $+commands[starship] )) && return 0

autoload -Uz add-zsh-hook

_git_prompt_segment=""
_update_git_prompt_segment() { _git_prompt_segment="$(git_custom_status)" }
add-zsh-hook precmd _update_git_prompt_segment

PROMPT='${_git_prompt_segment}%{$fg[cyan]%}[%~]%{$reset_color%}%B$%b '
