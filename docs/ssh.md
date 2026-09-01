## ssh archie / ssh macie

```
ssh archie
   ├── USB          10.77.77.1 → 10.77.77.2
   ├── direct Wi-Fi 10.77.78.1 → 10.77.78.2
   ├── regular LAN  filtered mDNS on 192.168.1.0/24
   └── Tailscale    100.75.71.79 → 100.126.231.24
```


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


```
dotfile system install --yes
sudo systemctl daemon-reload
sudo systemctl reload NetworkManager
```


## Connections

```
Macie <---USB C Cable (USB SuperSpeed Plus Gen 2x1, ~10Gbps , CDC-NCM Ethernet) ---> Archie

Archie <---Display Port Cable---> Monitor (Samsung LU28R55 4K IPS 60 Hz)
```



## Addressing the routes

```
ssh archie                 first reachable route in the four-path order
ssh 10.77.77.2             the cable, or nothing
ssh wifi-archie            direct Wi-Fi, or nothing
ssh lan-archie             filtered regular LAN, or nothing
ssh 100.126.231.24         tailscale, or nothing
```


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

```
hwire                 first reachable route in SSH order
hwire --all           every reachable route, one after the other
hwire -r wifi         direct Wi-Fi only, or fail
hwire -r lan          filtered regular LAN only, or fail
hwire -l              round trips only
```

## Inspecting the route

`hwire -i` reports connection state without running a benchmark. In a local
shell it probes all four routes concurrently, lists the routes that answer in
reverse preference order, and puts the route the next `ssh archie` or
`ssh macie` would choose last in bold color:

```console
$ hwire -i
TAILSCALE | LAN
```

Inside SSH it reads `SSH_CONNECTION` and reports the route carrying that
specific session. The result does not change merely because a better route was
plugged in after the session connected:

```console
$ hwire -i
LAN                                                             archie --> macie
```

The route order is cable, direct Wi-Fi, regular LAN, then Tailscale. `UNKNOWN`
means the server address could not be matched safely; `hwire` does not treat an
unrecognized private address as LAN.

Pass SSH names or addresses to inspect the configuration the next connection
would use. Every target is resolved independently with `ssh -G`:

```console
$ hwire -i archie lan-archie 100.126.231.24
```

`hwire -iv HOST...` adds the resolved hostname, binding or proxy, and
ControlMaster status, socket, age, and OpenSSH diagnostic. `hwire -i --json`
emits the same information as structured data. Use `--watch` to refresh the
route snapshot, `--interval SECONDS` to set its cadence, and `--notify` to ring
the terminal bell when the preferred route changes.

The old `hpath` shell function has been removed without an alias. Its explicit
target and JSON behavior now lives under `hwire -i`.
