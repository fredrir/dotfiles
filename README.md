# dotfiles

```
                  -`                    fredrir@archpc
                 .o+`                   --------------
                `ooo/                   OS: Arch Linux x86_64
               `+oooo:                  Shell: zsh
              `+oooooo:                 WM: KDE Plasma
              -+oooooo+:                Terminal: konsole
            `/:-:++oooo+:               Editor: neovim
           `/++++/+++++++:
          `/++++++++++++++:
         `/+++ooooooooooooo/`
        ./ooosssso++osssssso+`
       .oossssso-````/ossssss+`
      -osssssso.      :ssssssso.
     :osssssss/        osssso+++.
    /ossssssss/        +ssssooo/-
  `/ossssso+/:-        -:/+osssso+-
 `+sso+:-`                 `.-/+oso:
`++:.                           `-/+/
.`                                 `/
```


## Install

```bash
git clone https://github.com/fredrir/dotfiles ~/dotfiles
cd ~/dotfiles
./setup.sh desktop/arch-linux/kde            # archpc
./setup.sh laptop/arch-linux/hyprland        # laptop
./setup.sh macbook/macos                     # mac
```


## Adding a config

Groups: 
- `shared`
- `linux/common` 
- `linux/kde` 
- `linux/hyprland`
- `macos`

```bash
cd ~/dotfiles
mkdir -p <group>/<app>/.config/<app>
mv ~/.config/<app> <group>/<app>/.config/
grep -q '<group>' hosts/<profile>/manifest || echo '<group>' >> hosts/<profile>/manifest
./setup.sh <profile>
```

Placeholders:
- `<group>` → `linux/common`
- `<app>` → `nvim`
- `<profile>` → `desktop/arch-linux/kde`
- `<machine>` → `desktop`

## Adding a machine

1. Pick the matching profile under `hosts/<machine>/<distro>/<desktop>`

2. (Hyprland) put its monitors / GPU env / scale in
   `hosts/<profile>/hypr-local.conf`. See
   `hosts/desktop/arch-linux/kde-hyprland` (archpc) and
   `hosts/laptop/arch-linux/hyprland` (laptop).
3. `./setup.sh <profile>`

## Per-machine settings

- **Monitors / GPU / scale (Hyprland):** `hosts/<profile>/hypr-local.conf`
  (sourced last, overrides shared `conf.d/env.conf`).
- **Shell:** shared zsh fragments in `shared/zsh`; Linux-only in
  `linux/common/zsh`; Hyprland-only in `linux/hyprland/zsh`; macOS-only in
  `macos/zsh`. All co-stow into `~/.config/zsh/conf.d/` and load by numeric order.
- **Wallpaper:** place at `~/.config/hypr/wallpaper.png`

## Jetbrains

```bash
cd ~/dotfiles/linux/common
stow --adopt --no-folding --target="$HOME" --restow jetbrains
git -C ~/dotfiles checkout -- linux/common/jetbrains
/opt/WebStorm/bin/webstorm installPlugins \
    com.nasller.CodeGlancePro ru.adelf.idea.dotenv com.github.copilot \
    org.intellij.plugins.hcl "Key Promoter X"
```
