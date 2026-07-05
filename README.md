# dotfiles

<!-- fastfetch:start -->

```
                                                fredrir @ archpc
                                                ───────────────────────────────

                                                  SYSTEM
                                                󰣇  OS        Arch Linux x86_64
                        -`                      󰌽  Kernel    Linux 7.0.14-arch1-1
                       .o+`                     󰅐  Uptime    6 hours, 45 mins
                      `ooo/                     󰏗  Packages  1059 (pacman)
                     `+oooo:                    󰆍  Shell     zsh 5.9.1
                    `+oooooo:
                    -+oooooo+:                    HARDWARE
                  `/:-:++oooo+:                 󰻠  CPU       AMD Ryzen 7 9800X3D (16) @ 5.27 GHz
                 `/++++/+++++++:                󰢮  GPU       NVIDIA GeForce RTX 5070 Ti [Discrete]
                `/++++++++++++++:               󰍛  Memory    17 GB / 31 GB [54%]
               `/+++ooooooooooooo/`             󰋊  Disk      /  58 GB / 78 GB [75%]
              ./ooosssso++osssssso+`            󰋊  Disk      /home  75 GB / 118 GB [64%]
             .oossssso-````/ossssss+`
            -osssssso.      :ssssssso.            DESKTOP
           :osssssss/        osssso+++.         󰧨  DE        KDE Plasma 6.7.2
          /ossssssss/        +ssssooo/-         󰖯  WM        KWin (Wayland)
        `/ossssso+/:-        -:/+osssso+-       󰆌  Terminal  konsole 26.04.3
       `+sso+:-`                 `.-/+oso:      󰏘  Theme     Breeze (Dark) [Qt]
      `++:.                           `-/+/     󰍹  Display   2560x1440 in 27", 144 Hz [External] *
      .`                                 `/     󰍹  Display   2560x1440 in 27", 144 Hz [External]

                                                  NETWORK
                                                󰗊  Locale    en_US.UTF-8

                                                  ● ● ● ● ● ● ● ●
```

<!-- fastfetch:end -->

## Install

```bash
git clone https://github.com/fredrir/dotfiles ~/dotfiles
cd ~/dotfiles
./setup.sh desktop/arch-linux/kde            # archpc
./setup.sh laptop/arch-linux/hyprland        # laptop
./setup.sh macbook/macos                     # mac
```

## VPS / headless server

```bash
bash -c "$(curl -fsSL https://raw.githubusercontent.com/fredrir/dotfiles/main/bootstrap-vps.sh)"
```

Already cloned, or re-running

```bash
~/dotfiles/bootstrap-vps.sh            # full bootstrap
./setup.sh vps/linux                   # just re-stow the configs
```

## Theme

- **Palette:** `theme/palette.toml`

- **To regenerate theme:**

```bash
python3 scripts/generate-theme.py
```

## Adding a config

Placeholders:
- `<group>` → `linux/common`
- `<app>` → `nvim`
- `<profile>` → `desktop/arch-linux/kde`
- `<machine>` → `desktop`


 ```bash
cd ~/dotfiles
mkdir -p <group>/<app>/.config/<app>
mv ~/.config/<app> <group>/<app>/.config/
grep -q '<group>' hosts/<profile>/manifest || echo '<group>' >> hosts/<profile>/manifest
./setup.sh <profile>
```

## Adding a machine

1. Pick the matching profile under `hosts/<machine>/<distro>/<desktop>`
2. `./setup.sh <profile>`

## Jetbrains

```bash
cd ~/dotfiles/linux/common
stow --adopt --no-folding --target="$HOME" --restow jetbrains
git -C ~/dotfiles checkout -- linux/common/jetbrains
/opt/WebStorm/bin/webstorm installPlugins \
    com.nasller.CodeGlancePro ru.adelf.idea.dotenv com.github.copilot \
    org.intellij.plugins.hcl "Key Promoter X"
```
