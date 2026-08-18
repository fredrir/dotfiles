## ssh archie / ssh macie

ARCH:
NetworkManager → 10.77.77.2/30
dnsmasq DHCP  → gives Mac 10.77.77.1
no gateway / no DNS / no NAT

MAC:
zero persistent network hacks
built-in DHCP client on whatever enN the cable lands as
SSH Match exec chooses USB if reachable

ssh archie
   ├── USB available → 10.77.77.2, bound to 10.77.77.1
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

## Naming the cable

Neither end of this link has a stable name, and both used to be named.

The Mac's NCM address increments on every attach, and macOS keys
`NetworkInterfaces.plist` on the MAC, so each new address mints a new BSD
name: `en3`, `en4`, `en5`, and counting. The address the interface carries
does not move, because dnsmasq's range is one address wide.

On archie the same address churn broke the udev rename, which broke
everything keyed to the name after it. Both failures were silent — the probe
in `05-*` failed, so `ssh archie` took Tailscale and said nothing, and only
`echo $SSH_CONNECTION` or `hpath` gave it away.

So nothing names an interface any more. Both `05-*` probes bind their own end
with `nc -s`, and all four ssh files bind it again with `BindAddress`, which
gets the fallback right for free: the address exists only while the cable
does, so a failed bind means no cable rather than a stale name. The one name
left is `macie0`, which dnsmasq and NetworkManager key on — and the `.link`
file now derives it from the USB function rather than inheriting it from a
MAC, so it is an output of this configuration instead of an input from the
hardware.

When it does break, `udevadm test-builtin net_setup_link
/sys/class/net/<device>` says which `.link` file won on archie, and `hpath`
says which route the next ssh will take from either end.

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
│       │       └── 90-macie-usb-secondary.conf
│       └── systemd
│           ├── network
│           │   ├── 10-macie-usb.link
│           │   └── 11-macie-usb-secondary.link
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

## Measuring the two routes

`hwire` reports what either route is worth right now: round-trip latency and a
transfer each way, `--both` for the two side by side. It starts its own half
on the peer over ssh and binds this side's address for the route it is
measuring, so the numbers describe the cable or the tailnet rather than
whichever one the routing table preferred.

```
hwire            the cable when it is up, Tailscale when it is not
hwire --both     both, one after the other
hwire -l         round trips only
```
