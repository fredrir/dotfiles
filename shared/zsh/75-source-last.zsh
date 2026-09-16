for _plugin_file in $zsh_plugin_sources; do
	defer source "$_plugin_file"
done
unset _plugin_file

cached_eval starship-init starship init zsh
PROMPT_EOL_MARK='' # Fix extra % when no new-line

cached_eval direnv-hook direnv hook zsh
