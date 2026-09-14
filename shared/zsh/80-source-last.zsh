for _plugin_file in $zsh_plugin_sources; do
  defer source "$_plugin_file"
done
unset _plugin_file

cached_eval starship-init starship init zsh

cached_eval direnv-hook direnv hook zsh
