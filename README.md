# dotfiles

<!-- fastfetch:start -->

```
                                      fredrir @ macie
                                      ───────────────────────────────

                                        SYSTEM
                                      󰌽  OS        macOS Tahoe 26.5.1 (25F80) arm64
                        ..'           󰒓  Kernel    Darwin 25.5.0
                    ,xNMM.            󰅐  Uptime    3 hours, 38 mins
                  .OMMMMo             󰏗  Packages  229 (brew), 17 (brew-cask), 59 (nix-user), 51 (nix-default)
                  lMM"                󰆍  Shell     zsh 5.9
        .;loddo:.  .olloddol;.
      cKMMMMMMMMMMNWMMMMMMMMMM0:        HARDWARE
    .KMMMMMMMMMMMMMMMMMMMMMMMWd.      󰻠  CPU       Apple M5 Pro (5+10) @ 4.61 GHz
    XMMMMMMMMMMMMMMMMMMMMMMMX.        󰢮  GPU       Apple M5 Pro (16) @ 1.62 GHz [Integrated]
   ;MMMMMMMMMMMMMMMMMMMMMMMM:         󰍛  Memory    19 GB / 24 GB [78%]
   :MMMMMMMMMMMMMMMMMMMMMMMM:         󰋊  Disk      /  392 GB / 926 GB [42%]
   .MMMMMMMMMMMMMMMMMMMMMMMMX.
    kMMMMMMMMMMMMMMMMMMMMMMMMWd.        DESKTOP
    'XMMMMMMMMMMMMMMMMMMMMMMMMMMk     󰖯  WM        Quartz Compositor 1.600.0
     'XMMMMMMMMMMMMMMMMMMMMMMMMK.     󰆌  Terminal  ghostty
       kMMMMMMMMMMMMMMMMMMMMMMd       󰏘  Theme     Liquid Glass
        ;KMMMMMMMWXXWMMMMMMMk.        󰍹  Display   3024x1964 @ 2x in 14", 120 Hz [Built-in] *
          "cooc*"    "*coo'"          󰍹  Display   5120x2880 @ 2x in 29", 60 Hz [External]

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

## Syncing new changes

```bash
dotfile sync
```


## VPS / headless server

```bash
bash -c "$(curl -fsSL https://raw.githubusercontent.com/fredrir/dotfiles/main/bootstrap-vps.sh)"
# Or if cloned
./bootstrap-vps.sh
```


## Theme

- **Profiles:** `theme/profiles/*.toml`, assigned per group in `config/profiles.dotfile`

- **To switch profile:**

```bash
dotfile theme
```

- **To regenerate after editing a palette:**

```bash
dotfile theme apply
```

- **To see which profile each group uses:**

```bash
dotfile theme status
```

- **After a change that touches KDE:**

```bash
systemctl --user restart plasma-plasmashell
```

## The dotfile command

```bash
dotfile sync

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

dotfile system add /etc/dnsmasq-macie-usb.conf --pkg macie-usb
dotfile system status
dotfile system diff
dotfile system install
```

## Command-line tools

```bash
./setup.sh --commands-only
```



## Tests

```bash
tests/run.sh
tests/run.sh link
uv run --project scripts/python pytest
cargo test --manifest-path scripts/rust/Cargo.toml
```

## Adding a machine

1. Run `./setup.sh` and select your environment under `environment/<os>/<desktop>`
2. If the environment includes a group with an `overrides/` directory (machine-specific config such as `linux/hyprland/overrides/desktop` and `linux/hyprland/overrides/laptop`), pick the override that matches the machine, or `none`