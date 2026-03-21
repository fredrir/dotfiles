#!/bin/bash
set -e

DOTFILES="$(cd "$(dirname "$0")" && pwd)"

echo "Setting up dotfiles from $DOTFILES"

# Stow all packages
cd "$DOTFILES"
stow hypr nvim kitty waybar dunst wlogout zsh yazi

# Generate elephant config with $HOME expanded
mkdir -p "$HOME/.config/elephant"
sed "s|\$HOME|$HOME|g" "$DOTFILES/elephant/.config/elephant/files.toml" > "$HOME/.config/elephant/files.toml"
echo "Generated elephant config"

# Remind about local overrides
echo "Edit ~/.config/hypr/conf.d/local.conf for machine-specific settings (monitors, etc.)"

# Remind about wallpaper
if [ ! -f "$HOME/.config/hypr/wallpaper.png" ]; then
    echo "NOTE: Place your wallpaper at ~/.config/hypr/wallpaper.png"
fi

echo "Done!"
