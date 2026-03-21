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

# Create local hyprland override if it doesn't exist (not managed by stow)
if [ ! -f "$HOME/.config/hypr/conf.d/local.conf" ]; then
    cp "$DOTFILES/hypr/.config/hypr/conf.d/local.conf.example" "$HOME/.config/hypr/conf.d/local.conf"
    echo "Created local.conf - edit it for machine-specific settings (monitors, etc.)"
fi

# Remind about wallpaper
if [ ! -f "$HOME/.config/hypr/wallpaper.png" ]; then
    echo "NOTE: Place your wallpaper at ~/.config/hypr/wallpaper.png"
fi

hyprctl reload

echo "Done!"
