export ZSH_CUSTOM="$ZSH/custom"

# Homebrew.
if [ -x /opt/homebrew/bin/brew ]; then
    eval "$(/opt/homebrew/bin/brew shellenv)"
    elif [ -x /usr/local/bin/brew ]; then
    eval "$(/usr/local/bin/brew shellenv)"
fi

# Python
export PATH="$(brew --prefix python)/libexec/bin:$PATH"

# GNU Make
export PATH="/opt/homebrew/opt/make/libexec/gnubin:$PATH"
