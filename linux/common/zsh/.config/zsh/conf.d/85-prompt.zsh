# Render the git segment once per prompt instead of via command substitution
# inside PROMPT.
#
# The eastwood theme embeds $(git_custom_status) directly in PROMPT. With
# PROMPT_SUBST + zsh-autosuggestions + zsh-syntax-highlighting that substitution
# re-runs on every keystroke/redraw, and because its width varies (dirty marker,
# branch length, in/out of a repo) a redraw can leave a stray prompt fragment
# behind — e.g. a lingering gray "[" before the branch. Precomputing it in
# precmd keeps the rendered prompt a fixed-width string for the line's lifetime.
#
# Loads after oh-my-zsh.sh (which sets the eastwood PROMPT), so this overrides it
# while keeping the exact same look.
autoload -Uz add-zsh-hook

_git_prompt_segment=""
_update_git_prompt_segment() { _git_prompt_segment="$(git_custom_status)" }
add-zsh-hook precmd _update_git_prompt_segment

PROMPT='${_git_prompt_segment}%{$fg[cyan]%}[%~]%{$reset_color%}%B$%b '
