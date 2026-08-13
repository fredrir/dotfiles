# dotfiles

<!-- fastfetch:start -->

```
                                             fredrir @ archpc
                                             ───────────────────────────────

                                               SYSTEM
                                             󰌽  OS        Arch Linux x86_64
                     -`                      󰒓  Kernel    Linux 7.1.8-arch1-3
                    .o+`                     󰅐  Uptime    1 hour, 24 mins
                   `ooo/                     󰏗  Packages  1377 (pacman)
                  `+oooo:                    󰆍  Shell     zsh 5.9.2
                 `+oooooo:
                 -+oooooo+:                    HARDWARE
               `/:-:++oooo+:                 󰻠  CPU       AMD Ryzen 7 9800X3D (16) @ 5.27 GHz
              `/++++/+++++++:                󰢮  GPU       AMD Radeon Graphics [Integrated]
             `/++++++++++++++:               󰢮  GPU       NVIDIA GeForce RTX 5070 Ti [Discrete]
            `/+++ooooooooooooo/`             󰍛  Memory    5 GB / 31 GB [16%]
           ./ooosssso++osssssso+`            󰋊  Disk      /  71 GB / 366 GB [19%]
          .oossssso-````/ossssss+`           󰋊  Disk      /home  420 GB / 823 GB [51%]
         -osssssso.      :ssssssso.
        :osssssss/        osssso+++.           DESKTOP
       /ossssssss/        +ssssooo/-         󰧨  DE        KDE Plasma 6.7.4
     `/ossssso+/:-        -:/+osssso+-       󰖯  WM        KWin (Wayland)
    `+sso+:-`                 `.-/+oso:      󰆌  Terminal  kitty 0.48.2
   `++:.                           `-/+/     󰏘  Theme     Breeze (Light) [Qt]
   .`                                 `/     󰍹  Display   3840x2160 @ 1.5x in 29", 60 Hz [External]

                                               NETWORK
                                             󰗊  Locale    en_US.UTF-8

                                               ● ● ● ● ● ● ● ●
```

<!-- fastfetch:end -->

## Install

```bash
git clone https://github.com/fredrir/dotfiles ~/dotfiles
cd ~/dotfiles
./setup.sh
```


## VPS / headless server

```bash
bash -c "$(curl -fsSL https://raw.githubusercontent.com/fredrir/dotfiles/main/bootstrap-vps.sh)"
```

Already cloned, or re-running

```bash
~/dotfiles/bootstrap-vps.sh
```

## Theme

- **Palette:** `theme/palette.toml

- **To regenerate theme:**

```bash
generate-theme
```

```bash
systemctl --user restart plasma-plasmashell
```

## The dotfile command

```bash
dotfile add waybar
dotfile add --description "Status bar" waybar
dotfile add --linux zsh/conf.d/11-linux-env
dotfile add --kde konsolerc
dotfile add --pkg zsh ~/.zshrc
dotfile remove linux/common/fontconfig
dotfile remove /linux/server/zsh/conf.d/10-nvim.server.zsh

dotfile link
dotfile link arch-linux/kde
dotfile link --override linux/hyprland=laptop
dotfile link -n

dotfile status
dotfile check
dotfile check --all
dotfile packages
dotfile format

dotfile secret scan
dotfile secret scan --staged
dotfile secret scan --commits origin/main..HEAD

dotfile secret init
dotfile secret enroll archpc
dotfile secret keys
dotfile secret sync --rewrap
dotfile secret doctor

dotfile secret add ~/.ssh/config --pkg ssh
dotfile secret edit shared/ssh/config.enc
dotfile secret status
dotfile secret apply
dotfile secret clean

dotfile secret edit vars.enc.yaml
dotfile secret vars
dotfile secret vars --unused
```

## Command-line tools

```bash
./setup.sh --commands-only
```

The public commands are declared in `scripts/pyproject.toml` and installed as
an editable uv tool in `~/.local/bin`. The project environment remains
isolated from the shell PATH.

## Tests

```bash
tests/run.sh
tests/run.sh link
uv run --project scripts pytest
```

## Adding a machine

1. Run `./setup.sh` and pick the environment under `environment/<os>/<desktop>`
2. If the environment includes a group with an `overrides/` directory (machine-specific config such as `linux/hyprland/overrides/desktop` and `linux/hyprland/overrides/laptop`), pick the override that matches the machine, or `none`

## Jetbrains

New files WebStorm creates stay in `~/.config/JetBrains`

```bash
dotfile add --linux --pkg jetbrains JetBrains/WebStorm2026.1/options/editor.xml
/opt/WebStorm/bin/webstorm installPlugins \
    com.nasller.CodeGlancePro ru.adelf.idea.dotenv com.github.copilot \
    org.intellij.plugins.hcl "Key Promoter X"
```
