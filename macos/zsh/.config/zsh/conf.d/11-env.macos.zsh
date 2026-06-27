# macOS-only environment (loaded after the shared 10-env fragment).

# oh-my-zsh custom dir ($ZSH is set in 05-ohmyzsh.zsh).
export ZSH_CUSTOM="$ZSH/custom"

# Homebrew (Apple Silicon). Sets PATH/MANPATH/etc.
[ -x /opt/homebrew/bin/brew ] && eval "$(/opt/homebrew/bin/brew shellenv)"
