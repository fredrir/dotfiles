for _plugin_file in $zsh_plugin_sources; do
  source "$_plugin_file"
done
unset _plugin_file

(($+commands[starship])) && eval "$(starship init zsh)"

(($+commands[direnv])) && eval "$(direnv hook zsh)"
