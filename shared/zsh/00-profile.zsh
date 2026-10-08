# ~/.zprofile
# zsh-build: omit
(( ${+_zsh_build_rcs} )) && return

[[ -d $HOME/.docker/bin ]] && export PATH="$PATH:$HOME/.docker/bin"
[[ -x /opt/homebrew/bin/brew ]] && eval "$(/opt/homebrew/bin/brew shellenv zsh)"
[[ -r $HOME/.swiftly/env.sh ]] && source "$HOME/.swiftly/env.sh"
[[ -d $HOME/.local/bin ]] && export PATH="$HOME/.local/bin:$PATH"
[[ -d "$HOME/Library/Application Support/JetBrains/Toolbox/scripts" ]] && export PATH="$PATH:$HOME/Library/Application Support/JetBrains/Toolbox/scripts"
