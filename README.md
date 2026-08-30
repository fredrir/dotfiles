# dotfiles

<!-- fastfetch:start -->

```
                                      fredrir @ macie
                                      ───────────────────────────────

                                        SYSTEM
                                      󰌽  OS        macOS Tahoe 26.6.2 (25G83) arm64
                        ..'           󰒓  Kernel    Darwin 25.6.0
                    ,xNMM.            󰅐  Uptime    50 mins
                  .OMMMMo             󰏗  Packages  234 (brew), 18 (brew-cask)
                  lMM"                󰆍  Shell     zsh 5.9
        .;loddo:.  .olloddol;.
      cKMMMMMMMMMMNWMMMMMMMMMM0:        HARDWARE
    .KMMMMMMMMMMMMMMMMMMMMMMMWd.      󰻠  CPU       Apple M5 Pro (5+10) @ 4.61 GHz
    XMMMMMMMMMMMMMMMMMMMMMMMX.        󰢮  GPU       Apple M5 Pro (16) @ 1.62 GHz [Integrated]
   ;MMMMMMMMMMMMMMMMMMMMMMMM:         󰍛  Memory    18 GB / 24 GB [74%]
   :MMMMMMMMMMMMMMMMMMMMMMMM:         󰋊  Disk      /  388 GB / 926 GB [42%]
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

./setup.sh --commands-only # CLI only
```

## Switch theme profile

```bash
dotfile theme
```