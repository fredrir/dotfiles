# macOS-only environment (loaded after the shared 10-env fragment).

# oh-my-zsh custom dir ($ZSH is set in 05-ohmyzsh.zsh).
export ZSH_CUSTOM="$ZSH/custom"

# Homebrew. Sets PATH/MANPATH/etc before later fragments look up tools like
# starship.
if [ -x /opt/homebrew/bin/brew ]; then
  eval "$(/opt/homebrew/bin/brew shellenv)"
elif [ -x /usr/local/bin/brew ]; then
  eval "$(/usr/local/bin/brew shellenv)"
fi
