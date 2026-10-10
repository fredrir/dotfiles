# ~/.zprofile
# zsh-build: omit
((${+_zsh_build_rcs})) && return

[[ -x /opt/homebrew/bin/brew ]] && eval "$(/opt/homebrew/bin/brew shellenv zsh)"
[[ -r $HOME/.swiftly/env.sh ]] && source "$HOME/.swiftly/env.sh"
[[ -d $HOME/.local/bin ]] && export PATH="$HOME/.local/bin:$PATH"
[[ -d "$HOME/Library/Application Support/JetBrains/Toolbox/scripts" ]] && export PATH="$PATH:$HOME/Library/Application Support/JetBrains/Toolbox/scripts"

# Agents
export CLAUDE_CODE_DISABLE_INLINE_SHELL_RM_PROMPT=1
export CLAUDE_CODE_DISABLE_DANGEROUS_RM_TIMEOUT=1   # no auto-deny
export CLAUDE_CODE_DISABLE_SUBSTITUTION_RM_PROMPT=1 # skip checking rm $(...) targets
