## ssh archie / ssh macie

OpenSSH resolves the peer through four honest, ordered routes:

```
ssh archie
   ├── USB          10.77.77.1 → 10.77.77.2
   ├── direct Wi-Fi 10.77.78.1 → 10.77.78.2
   ├── regular LAN  filtered mDNS on 192.168.1.0/24
   └── Tailscale    100.75.71.79 → 100.126.231.24
```

USB and direct Wi-Fi bind their source addresses. The LAN ProxyCommand accepts
`archpc.local`/`macie-2.local` only when both the resolved peer and the source
chosen by the kernel are in `192.168.1.0/24`. Tailscale uses numeric tailnet
addresses. A route label therefore cannot answer over a different transport.

SSH only selects among networks that already exist. It never associates
Macie's Wi-Fi with `archie-direct`; that global transition belongs to the
explicit `archie-direct` controller below.


## Behavior

```
Start ssh archie with cable and archie-direct absent
    ↓
SSH connects via regular LAN (or Tailscale when away from home)
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


| Event                              | Existing SSH                  | Next `ssh archie` |
| ---------------------------------- | ----------------------------- | ----------------- |
| Home Wi-Fi, no cable/direct AP     | Current connection continues  | **LAN**           |
| Join `archie-direct`, cable absent | Current connection continues  | **direct Wi-Fi**  |
| Plug cable in                      | Existing connection unchanged | **USB**           |
| Leave home and direct AP           | Dead session stays dead       | **Tailscale**     |

## Direct Wi-Fi AP

Nothing starts at boot. On Macie, use:

```
archie-direct enroll                 # one-time WPA3/Keychain enrollment
archie-direct start shared           # AP + routed Internet via Archie
archie-direct status [--json]
archie-direct benchmark baseline|shared
archie-direct stop
```

A complete comparison is intentionally manual and leaves the decision to the
results:

```
archie-direct enroll
archie-direct stop
archie-direct benchmark baseline
archie-direct start shared
archie-direct benchmark shared
archie-direct stop
```

Each benchmark performs five one-stream and five four-stream transfers in
both directions, a 200-packet ping run, radio/PHY capture, and DNS/Internet
checks. It writes raw JSON plus `summary.{json,md}` below
`~/.local/state/archie-direct/benchmarks/`; it does not select or enable a
winning mode.

The Archie-side prerequisite is installed with the rest of the Arch profile.
After changing its tracked root-owned files, apply them and reload their
consumers:

```
dotfile system install --yes
sudo systemctl daemon-reload
sudo systemctl reload NetworkManager
```

Shared mode creates `archie0` on the live non-DFS 5 GHz home channel. Macie
gets `10.77.78.1/30`, Archie is its IPv4 gateway/DNS at `10.77.78.2`, and a
dedicated nftables table forwards and masquerades only through `wlp9s0`.
When Docker's iptables backend owns a later `FORWARD` chain, two scoped
`DOCKER-USER` rules admit only `archie0` outbound traffic and its established
replies; stop and failure cleanup remove them.

An isolated same-radio AP is intentionally not exposed. The tested 6 GHz/160
MHz and 5 GHz/80 MHz variants associated, but did not provide a reliable IP
data path between the MT7925 and macOS. Shared mode is the supported mode.

The password is a SOPS variable rendered only to Archie's root-readable
hostapd configuration. Enrollment puts it on Macie's clipboard only long
enough for the manual first join, then restores the previous clipboard.

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

## Addressing the routes

`ssh archie` is an answer to "get me to archie", and it is deliberately
evasive about which wire it used. That is right for a person and wrong for
anything that has to record what it did. So every route has a spelling
that cannot change its mind:

```
ssh archie                 first reachable route in the four-path order
ssh 10.77.77.2             the cable, or nothing
ssh wifi-archie            direct Wi-Fi, or nothing
ssh lan-archie             filtered regular LAN, or nothing
ssh 100.126.231.24         tailscale, or nothing
```

and the mirrored spellings from Archie: `macie`, `10.77.77.1`, `wifi-macie`,
`lan-macie`, and `100.75.71.79`.

Two things wanted this. dmux enrolls one route per address and labels each
one, and a route it has labelled `usb` must not answer over tailscale when
the cable is out — that label is what licenses remote Wez, and there is no
rule for automatic Wez over the tailnet. Under the alias the label is a
guess: `05-`'s probe decides after dmux has already written the row down.
The other is wezterm, whose built-in ssh client gets the route's address
verbatim and does not implement `Match exec` at all; `wez/remote/mux.lua`
has named these addresses outright for exactly that reason since before dmux.

The `05-*`, `06-*`, and `07-*` probes populate the alias in order. Cable and
direct Wi-Fi use tight keepalives and bound addresses; LAN uses the filtered
mDNS connector; Tailscale keeps the slack roaming keepalives.

Every one of these carries `HostKeyAlias archie` (or `macie`). Without it,
each new spelling would mint its own `known_hosts` entry, and archie would be
four hosts in that file instead of one — which is the thing the unification
went to some trouble to stop. With it, `ssh 10.77.77.2` on an empty
`known_hosts` writes `archie`; with `-F /dev/null` the same connection writes
`10.77.77.2`. That is the whole difference, and it is worth checking after
any change here:

```
ssh -G 10.77.77.2 | grep -i hostkeyalias
ssh -o BatchMode=yes -o ConnectTimeout=6 fredrir@10.77.77.2 true; echo $?
```

## Relevant files

```
# Macie
macos
├── ssh
│   └── config.d
│       ├── 05-archie-cabled-first
│       ├── 06-archie-wifi-first
│       ├── 07-archie-lan-first
│       ├── 10-archie-tailscale
│       ├── 40-cabled
│       ├── 41-wifi
│       └── 42-lan
└── zsh
    └── conf.d
        └── 92-archie-direct.zsh


# Archie
/home/fredrir/dotfiles/linux/arch
├── archie-direct
│   ├── etc
│   │   ├── archie-direct
│   │   ├── NetworkManager/conf.d
│   │   └── systemd/system
│   └── usr/local/libexec/archie-direct-host
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
│       ├── 06-macie-wifi-first
│       ├── 07-macie-lan-first
│       ├── 10-macie-tailscale
│       ├── 40-cabled
│       ├── 41-wifi
│       └── 42-lan
├── wezterm-mux
│   └── wezterm-mux.service
└── zsh
    └── conf.d
        └── 21-paths.arch.zsh
```

## Measuring the routes

`hwire` reports what each route is worth right now: round-trip latency and a
transfer each way, `--all` for every reachable route. It starts its own half
on the peer over ssh and binds this side's address for the route it is
measuring, so the numbers describe the selected cable, Wi-Fi, LAN, or tailnet
route rather than whichever one the routing table preferred.

```
hwire                 first reachable route in SSH order
hwire --all           every reachable route, one after the other
hwire -r wifi         direct Wi-Fi only, or fail
hwire -r lan          filtered regular LAN only, or fail
hwire -l              round trips only
```

`--both`/`-b` remains a compatibility alias for `--all`.

## Mosh

Mosh is an optional interactive client, not a selector. Archie already has
the server; Macie's requirements install the client. Start it with an explicit
route alias when route identity matters, for example `mosh wifi-archie` or
`mosh lan-archie`. It uses SSH for authentication and then UDP 60000–61000.
It can improve typing under jitter and survive client roaming, but it does not
carry SCP/rsync, SSH forwarding, or WezTerm's native mux transport. dmux and
WezTerm deliberately keep their existing USB/Tailscale policy in this phase.
