## ssh archie / ssh macie

ARCH:
NetworkManager → 10.77.77.2/30
dnsmasq DHCP  → gives Mac 10.77.77.1
no gateway / no DNS / no NAT

MAC:
zero persistent network hacks
built-in en3 DHCP client
SSH Match exec chooses USB if reachable

ssh archie
   ├── USB available → 10.77.77.2 via en3
   └── USB absent    → archie via Tailscale


## Behavior

```
Start ssh archie with cable absent
    ↓
SSH connects via Tailscale
    ↓
plug cable in
    ↓
existing SSH session stays on Tailscale

Start another ssh archie
    ↓
probe sees 10.77.77.2:22
    ↓
new session uses USB
```


| Event                          | Existing SSH                  | Next `ssh archie` |
| ------------------------------ | ----------------------------- | ----------------- |
| Cable absent                   | Tailscale continues           | Tailscale         |
| Plug cable in                  | Existing connection unchanged | **USB**           |
| USB session open, unplug cable | USB session eventually dies   | **Tailscale**     |
| Plug cable back in             | Dead session stays dead       | **USB**           |

## Connections

```
Macie <---USB C Cable (USB SuperSpeed Plus Gen 2x1, ~10Gbps , CDC-NCM Ethernet) ---> Archie

Archie <---Display Port Cable---> Monitor (Samsung LU28R55 4K IPS 60 Hz)
```

## Relevant files

```
# Macie
macos
├── ssh
│   └── config.d
│       ├── 05-archie-cabled-first
│       ├── 10-archie-tailscale
│       ├── 30-distro-lab-remote
│       └── 40-cabled
├── sunshine
│   ├── apps.json
│   └── sunshine.conf
└── zsh
    └── conf.d
        ├── 11-env.macos.zsh
        ├── 21-paths.macos.zsh
        ├── 41-aliases.mac.zsh
        └── 80-plugins.macos.zsh


# Archie
/home/fredrir/dotfiles/linux/arch
├── macie-usb
│   └── etc
│       ├── dnsmasq-macie-usb.conf
│       ├── NetworkManager
│       │   └── conf.d
│       │       └── 90-macie-usb-secondary.conf.tmpl
│       └── systemd
│           ├── network
│           │   ├── 10-macie-usb.link.tmpl
│           │   └── 11-macie-usb-secondary.link.tmpl
│           └── system
│               └── macie-usb-dhcp.service
├── ssh
│   └── config.d
│       ├── 05-macie-cabled-first
│       ├── 10-macie-tailscale
│       └── 40-cabled
├── wezterm-mux
│   └── wezterm-mux.service
└── zsh
    └── conf.d
        └── 21-paths.arch.zsh
```