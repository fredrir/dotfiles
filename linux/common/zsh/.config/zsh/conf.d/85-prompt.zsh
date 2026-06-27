# Precompute the git segment in precmd instead of $(git_custom_status) inside PROMPT.
# Under PROMPT_SUBST + autosuggestions/syntax-highlighting that re-runs every keystroke,
# and its varying width can leave stray prompt fragments. Loads after oh-my-zsh.sh,
# overriding eastwood's PROMPT with the same look.
autoload -Uz add-zsh-hook

_git_prompt_segment=""
_update_git_prompt_segment() { _git_prompt_segment="$(git_custom_status)" }
add-zsh-hook precmd _update_git_prompt_segment

PROMPT='${_git_prompt_segment}%{$fg[cyan]%}[%~]%{$reset_color%}%B$%b '
