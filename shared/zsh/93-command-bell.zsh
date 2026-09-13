# Bell when a foreground command ran >= 30s
zmodload zsh/datetime
autoload -Uz add-zsh-hook

__long_cmd_start=0
__long_cmd_preexec() { __long_cmd_start=$EPOCHSECONDS; }
__long_cmd_precmd() {
  ((__long_cmd_start)) || return 0
  ((EPOCHSECONDS - __long_cmd_start >= 30)) && printf '\a'
  __long_cmd_start=0
}
add-zsh-hook preexec __long_cmd_preexec
add-zsh-hook precmd __long_cmd_precmd
