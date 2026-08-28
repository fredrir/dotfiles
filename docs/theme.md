# dotfile theme

theme profile consists of a custom colour palette and fonts. 

`config/profiles.dotfile` declares which profile each group uses, and allows for package spesific overides. 

### config/profiles.dotfile

```
shared {
  theme    = mocha
  obsidian = latte
}

linux/kde {
  theme = latte
}
```

| group                                 | owns                                                     |
| ------------------------------------- | -------------------------------------------------------- |
| `shared`                              | kitty, wezterm, starship, zsh, obsidian, nvim, fastfetch |
| `linux/common`                        | GTK colours and settings, quicklaunch                    |
| `linux/kde`                           | kdeglobals, desktop-appletsrc, konsole, panel presets    |
| `linux/arch`, `linux/ubuntu`, `macos` | that platform's fastfetch config and logo                |