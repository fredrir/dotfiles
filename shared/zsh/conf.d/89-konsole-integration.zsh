[[ -o interactive ]] || return 0
[[ -n ${KONSOLE_DBUS_SESSION:-} ]] || return 0

autoload -Uz add-zsh-hook

_konsole_osc133_preexec() {
  builtin print -n -- $'\e]133;C\a' > /dev/tty
}

add-zsh-hook -d preexec _konsole_osc133_preexec 2>/dev/null
add-zsh-hook preexec _konsole_osc133_preexec

_konsole_osc133_prompt_start=$'%{\e]133;L\a\e]133;D;%?\a\e]133;A\a%}'
_konsole_osc133_secondary_start=$'%{\e]133;A\a%}'
_konsole_osc133_prompt_end=$'%{\e]133;B\a%}'

[[ $PROMPT == "$_konsole_osc133_prompt_start"* ]] || PROMPT="$_konsole_osc133_prompt_start$PROMPT"
[[ $RPROMPT == *"$_konsole_osc133_prompt_end" ]] || RPROMPT="$RPROMPT$_konsole_osc133_prompt_end"
[[ $PROMPT2 == "$_konsole_osc133_secondary_start"* ]] || PROMPT2="$_konsole_osc133_secondary_start$PROMPT2"
[[ $PROMPT2 == *"$_konsole_osc133_prompt_end" ]] || PROMPT2="$PROMPT2$_konsole_osc133_prompt_end"

unset _konsole_osc133_prompt_start
unset _konsole_osc133_secondary_start
unset _konsole_osc133_prompt_end
