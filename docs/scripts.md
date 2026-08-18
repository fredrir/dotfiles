# scripts

Reference for the workstation command-line tools in `scripts/`. Code files
carry no comments, so the reasoning behind the non-obvious behaviour lives
here.

`scripts/` holds two projects: `scripts/python`, the uv-managed Python
project, and `scripts/rust`, a cargo workspace for native tools — the ones
where interpreter overhead would distort what is being measured, and the
small commands that run often enough that a Python start was most of their
cost.

`scripts/python` is a uv-managed Python project (Typer + Rich, package name
`tools`). `uv sync --project scripts/python --locked` installs its development
environment into `scripts/python/.venv`. Setup also installs the project as an
editable uv tool and exposes only the console entry points declared in
`scripts/python/pyproject.toml` through `~/.local/bin`, with runtime dependencies
constrained by `scripts/python/uv.lock`. Setup enables install-time bytecode
compilation for both environments. Re-running setup reconciles that directory
with the declaration, including pruning removed entry points.

Setup is incremental: the tool reinstall is keyed on a hash of
`pyproject.toml` + `uv.lock` (the install is editable, so source edits need no
reinstall), and the cargo build on a hash of the `scripts/rust` sources, with
stamps under `~/.config/dotfile/sync`. `dotfile sync` runs `setup.sh --sync`:
the same steps non-interactively with the saved environment and overrides, so
the routine after a pull is one command that only does the work the pull
created. When run from the configured zsh, a successful sync replaces the
current shell with a fresh zsh process so linked shell changes take effect
immediately; a failed sync leaves the existing shell alone. A first run on a
new machine is `./setup.sh`, which is the same script with the pickers. The Linux,
macOS, and Ubuntu zsh profiles put `~/.local/bin` on PATH without exposing the
project environment's Python or dependency commands. The Ubuntu VPS bootstrap
uses `./setup.sh --commands-only` to install the same entry points without a
development environment.

## Layout

```
scripts/python/
  pyproject.toml             project, dependencies, console entry points
  uv.lock                    locked dependency versions
  src/tools/
    core/
      blocks.py              shared `.dotfile` block grammar scanner
      console.py             shared output, errors, color gating
      menu.py                arrow-key picker, with optional live preview
      paths.py               repository root discovery, ~ shortening
      process.py             subprocess helpers
      typography.py          terminal-safe block lettering
    utils/
      tardirs.py             tar archive directory tree with entry counts
      remote_clipboard.py    text clipboard transfer between macOS and Archie
      sysinfo/               modular hardware and software summary
        cli.py               pretty, full and health command flags
        collect.py           native collector, fastfetch and NVIDIA probes
        models.py            shared snapshots, components and render options
        branding.py          vendor registry, marks, colours and fallbacks
        profiles.py          exact-model facts and operating limits
        devices.py           shared device normalization and telemetry helpers
        facts.py             shared normalized fact construction
        formatting.py        shared units, percentages and text formatting
        identity.py          platform-aware username and hostname resolution
        hosts.py             config/hosts.dotfile parsing and host resolution
        bench/
          cli.py             bench subcommands and their menus
          record.py          run and metric schema, epoch derivation
          store.py           run files, baselines, lock, retention
          capture.py         embedded hardware snapshot and install facts
          conditions.py      confounders, gating and run grading
          runner.py          orchestration and dynamic run counts
          suites/            one module per measurement family; native.py
                             drives the bench-workloads Rust binary
          select.py          host[/os][@epoch][:run] selectors
          compare.py         deltas, noise bands, snapshot diffs
          health.py          benchmark findings as HealthIssues
          report.py          run, list, comparison and trend rendering
          document.py        benchmarks/BENCHMARKS.md generation
        normalization.py     platform and device sanitation helpers
        hardware.py          normalized hardware components
        software.py          normalized software and system facts
        view.py              compact orchestration of the shared view
        health.py            error and warning evaluation
        plain.py             compact and full plain-text rendering
        pretty.py            responsive borderless Rich rendering
    desktop/
      power_menu.py          wofi power menu (Hyprland)
      confirm_exit.py        wofi exit confirmation (Hyprland)
      clean_copy.py          clipboard normaliser behind Ctrl+Shift+C
    readme/
      fastfetch.py           fastfetch preview block in README.md
    theme/
      model.py               profile loading, merging, colour conversion
      profiles.py            config/profiles.dotfile: reading it, and rewriting it
      validate.py            what a profile must define to be usable
      render.py              file writing and in-place editing
      emitters.py            one function per generated config
      registry.py            emitter list and their declared outputs
      view.py                swatches, profile cards, status and previews
      cli.py                 theme subcommands and their menus
    dotfile/
      cli.py                 command dispatch
      state.py               repo context, profiles, overrides, manifests
      targets.py             config/targets.dotfile -> destination mapping
      link.py                link / unlink / prune engine
      check.py               health of the installed environment
      packages.py            config/packages.dotfile and PACKAGES.md
      add.py                 adopt a live config into the repo
      remove.py              stop tracking a path, keep it live
      format.py              .conf formatter
      profiles.py            host platform and desktop detection
      report.py              shared marks, alignment and clipping
      system.py              root-owned files under /etc
      secret/
        cli.py               secret subcommand dispatch
        scan.py              pattern, canary and invariant tiers
        canaries.py          private strings from the local file and vars
        allow.py             config/scan.dotfile false-positive allowlist
        identity.py          this machine's age identity
        keys.py              config/keys.dotfile -> .sops.yaml
        manage.py            init, enroll, revoke, sync
        vault.py             encrypt, decrypt, render, materialise
        variables.py         vars.enc.yaml loading and template rendering
        store.py             add, edit, vars listing
        apply.py             apply, status, clean and their reporting
        doctor.py            end-to-end health of the secret system
  tests/                     pytest suites per area
scripts/rust/
  Cargo.toml                 cargo workspace, release codegen pinned for
                             reproducible measurement binaries
  rust-toolchain.toml        toolchain channel and components
  crates/
    bench-workloads/         dependency-free native workloads for sysinfo bench
    count/                   count items inside a directory
    dmux/                    wezterm-mux + tmux session manager
    flatten/                 undo nesting, or bring a whole subtree up
    git/
      gdd/                   discard every change in the working tree
      gitkit/                shared repository access, survey, plan and discard
      gpp/                   git add + commit + push
    path/                    repo-relative or home-relative path of a target
    size/                    sizes and line counts for files and directories
    sysinfo-collect/         native system probing, fastfetch-shaped JSON
    workstation/             shared completions flag, palette, prompt and
                             error convention
tests/
  run.sh                     black-box test runner, plus cargo test
  lib.sh                     sandbox and assertions
  cases/                     one file per test group
```

Data that used to be embedded in the programs now sits next to the palette:

```
theme/
  profiles/<name>.toml       one profile: colours and what it overrides
  roles.toml                 semantic colour layers every profile inherits
  fonts.toml                 font roles, sizes and per-application opt-in
  maps/gtk.toml              GTK @define-color name -> role
  maps/kde.toml              KColorScheme groups, foregrounds, selection
  maps/eza.toml              file-type category -> extensions
  maps/catppuccin.toml       upstream Catppuccin hex -> palette name
  maps/obsidian.toml         Obsidian CSS custom property -> colour
```

`theme/` holds colour and font data only, and says nothing about which profile
is in force — that is `config/profiles.dotfile` at the repository root, beside the
other `.dotfile` declarations. Every config that carries colour is a normal
tracked dotfile in its own package; the generator stamps values into it rather
than rendering it from a template.

## sysinfo

`sysinfo` prints a compact, uncoloured hardware and software identity. `-p` and
`--pretty` select the branded terminal presentation with the complete hardware
inventory, including clocks, caches, thermals and utilization. In plain mode,
`-f` and `--full` expand the entire inventory. Combined with pretty mode, full
adds the software and system inventories beneath the hardware presentation.
`-hh` and `--health` reveal diagnostic explanations and actions. The switches
are independent and may be combined.

The main view reports only the number of active errors and warnings. A healthy
machine has no health line or empty health section. Diagnostic prose is never
shown unless health mode is requested. Swap status is factual in full mode and
becomes a warning only when high memory pressure makes the missing fallback
actionable.

The primary detector is `sysinfo-collect`, the Rust binary in `scripts/rust`:
sysctl and IOKit on macOS, /proc and /sys with the hwdata/libdrm ID databases
on Linux, emitting the same fastfetch-shaped module JSON the readers have
always consumed. Its field values are matched against what fastfetch reports
on the same hardware — the benchmark epoch is derived from them, and this was
verified against the pinned baselines on both archie and macie before the
switch. When the binary is missing, collection falls back to fastfetch
unchanged; `SYSINFO_COLLECTOR` overrides the binary path for tests. In full
mode, the purely cosmetic modules the native collector does not implement
(Host, Packages, Theme, Display, OpenCL, Vulkan, firmware) merge in from
fastfetch when it is installed and silently stay absent when it is not.

Targeted NVIDIA telemetry enriches matching devices with live VRAM,
utilization, clock and power readings from `nvidia-smi`. Optional probe
failures become health findings and never prevent the remaining snapshot from
rendering. Static components such as the cooler, memory kit, case and power
supply come from `config/hosts.dotfile` when firmware interfaces cannot expose
them without elevated privileges.

### config/hosts.dotfile

One block per physical machine, in the same grammar as `config/packages.dotfile`:

```
archie {
  hostnames = archpc, archie, archie.local
  role = desktop

  CPU_COOLER = Noctua NH-D15
  MEMORY = Corsair CMK32GX5M2B6000Z30 32 GB (2×16 GB) DDR5-6000 CL30
}
```

This replaced `hardware.dotfile`, which was keyed by *platform* and defaulted to
`desktop` for any Linux machine. That meant the Ubuntu VPS reported the
workstation's cooler, memory kit, case and power supply as its own. Keying by
host fixes it: a machine that matches no block gets no configured hardware
rather than someone else's.

Identity resolves in order — `--host`, `$SYSINFO_HOST`,
`~/.config/dotfile/host`, then a `hostnames` match — because the hostname is not
the name we use for a machine (this workstation answers to `archpc` but is
called `archie` everywhere else). `hostnames` is a list so a reinstall under a
different name still maps to the same lineage. `$SYSINFO_CONFIG` overrides the
file location for tests.

Distro, kernel and driver are deliberately not declared. They change without
anyone editing the file and are already detected at run time.

Brand detection has three layers: exact-model profiles for verified limits,
vendor and product-family profiles for presentation, then device-class
fallbacks. Replacing a known GPU or CPU with a future model retains its vendor
identity without requiring an exact model entry. Unknown manufacturers remain
readable and never stop rendering.

Pretty output is left aligned and borderless. Wide terminals use an invisible
two-column hardware grid, narrow terminals stack the same components, and
limited terminals retain text labels when an icon is unavailable. Colours
honour `NO_COLOR` and disappear when stdout is redirected.

Device serials, display identifiers, network addresses and Wi-Fi names are
never copied into the normalized view or rendered. The title retains the local
username and hostname, matching the existing Fastfetch presentation.

## sysinfo bench

Measures this machine and persists each run so they can be compared over time
and across machines. Bare `sysinfo bench` opens the same arrow-key menu the
`transcript` command uses; every entry has a non-interactive equivalent.

```
sysinfo bench run [--tier quick|standard|heavy] [--only cpu,mem] [--note "..."]
sysinfo bench show [<selector>]          sysinfo bench compare <left> <right>
sysinfo bench list [--host archie]       sysinfo bench trend archie cpu.multi
sysinfo bench health                     sysinfo bench baseline set|clear|show
sysinfo bench prune [--dry-run] [--yes]  sysinfo bench document
```

`disk` and `thermal` produce no jobs at the default `quick` tier, so
`--only disk` needs `--tier standard` or it measures nothing. The menu entries
that pick interactively — `compare`, `show`, `trend`, `baseline set` — need a
terminal; piped, they say so and exit non-zero rather than printing nothing.
`prune` deletes, so it confirms first, and needs `--yes` when unattended.

### What a run is pinned to

Four independent dimensions, because each question holds a different set fixed:
the **host** (declared in `config/hosts.dotfile`), the **hardware snapshot** embedded in
the run, the **install** (distro, kernel, driver) captured automatically, and the
**run** itself (time, tier, note, conditions).

Every run embeds its resolved hardware snapshot rather than pointing at one.
`config/hosts.dotfile` describes *now*; a run describes *then*. So there are no
hardware revision numbers to maintain, `compare` can diff two embedded snapshots
and report "GPU changed: RTX 3080 → RTX 5070 Ti" on its own, and a run taken
inside a VM is self-evident from its own record.

The **epoch** (`10db7d1f`) is a derived index key, not stored state: a blake2s
digest over the identity-bearing snapshot fields. Swap a part and it changes by
itself, while older runs keep their old parts. `git log -p -- config/hosts.dotfile` is
the upgrade log; nothing else records it.

Which fields count is a deliberately narrow question, because anything that
drifts on its own silently orphans the pinned baseline — after which a real
regression is reported as "no findings", quietly. So capacities are compared at
whole-GiB resolution rather than raw (`nvidia-smi` reports VRAM as a float and
the fastfetch fallback as an int, and the two disagree by a few hundred MiB);
device lists are sorted, because enumeration order is not identity; removable
media is excluded at capture; and memory *module count* is excluded entirely,
since it reads 0 without root and 2 with, so a single `sudo` run would otherwise
change the machine's identity.

### Why runs are stored one file per run

`benchmarks/<host>/<timestamp>-<epoch>.json`, committed. Committing is what makes
cross-machine comparison work at all, since git already syncs the two machines.
One file per run means two machines can record independently and merge without
ever touching the same bytes. `baselines.dotfile` pins the reference run per host
and epoch, and `BENCHMARKS.md` is generated from the set by `sysinfo bench
document`, staged by `pre-commit` — the same relationship `config/packages.dotfile` has
with `PACKAGES.md`.

Records carry the declared host alias only. Serials, `machine-id`, disk WWNs,
MAC addresses and usernames are never written, because these files are public.

### Samples, not a score

Every metric stores its samples and reports median and MAD. That is what lets a
comparison say **"within noise"**, which is the honest answer most of the time
and one a single number can never give. Repetition follows the Phoronix Test
Suite's DynamicRunCount: three runs, continuing to six while relative standard
deviation stays above 2.5%.

Tools that already average internally — `glmark2`, `vkmark`, `fio`, `hyperfine` —
are run once and not repeated, because external repetition of an internally
averaged result buys nothing and costs minutes.

Metrics are self-describing in the PTS vocabulary: `scale` (`ResultScale`),
`proportion` (`Proportion`, `HIB` or `LIB`) and `times_to_run`. `method` carries
`name/X.Y.Z` semantics where a major or minor bump means results are **not**
comparable.

`comparable` records how far a number travels: `world` (any machine running the
same tool version), `platform` (one OS family), or `host` (this machine's own
history only). GPU results are always `host` — a CUDA score and a Metal score
are not the same measurement, and `compare` refuses to present them as one.

`disk.*` and `thermal.*` are `host` too. They were `platform`, but `platform`
only blocks when `install["os"]` differs, and that is a distro id, not a piece
of hardware: two Arch machines with entirely different SSDs and coolers had
their numbers compared head to head and labelled better or worse.

This answers "has archie changed relative to archie?" It cannot answer "is my
9800X3D performing like other 9800X3Ds?", because there is no population to
compare against. There is deliberately no composite score: a single number
spanning x86-64/CUDA and arm64/Metal would be numerology.

Metrics that run once — `disk.*`, `gpu.*`, `thermal.*` — get a wider noise band
than replicated ones. A single sample has no spread, so its band would otherwise
collapse onto the 2% floor, and `fio` and `glmark2` do not repeat to 2%. The
unreplicated metrics would have produced the most false verdicts.

### What the memory metrics actually measure

`mem.*` and `cache.*` are the same tool pointed at two different things, and the
split exists because conflating them is easy and produces a number that cannot
be true.

sysbench sizes its working set from `--memory-block-size`; `--memory-total-size`
only sets how many times that buffer is traversed. At a 1 MiB block the buffer
sits inside L2 on essentially every current part, so what comes back is cache
bandwidth. Published as `mem.read` and scoped `world`, that invites comparison
against machines where the same figure means something else entirely — and on a
wide x86 core it can exceed what the memory bus can physically carry.

So `mem.*` uses a 1 GiB block — ten times the largest current L3, because a
buffer only a couple of times the cache still reads partly out of it: on the
96 MiB X3D part a 256 MiB block measures 15% faster than a 1 GiB one. `cache.*`
keeps the 1 MiB block under a name that says what it is, and is `host`-scoped:
where a 1 MiB buffer lands depends on a particular cache hierarchy, so the
number says nothing held against a different design.

**Both are single threaded, and that is the interesting decision.** Threading
looks obviously right — one core cannot saturate a memory controller, and on
Apple Silicon a single scalar load loop is issue-bound around 34 GB/s, well
below both cache and DRAM. But measured on the 9800X3D, 16 threads at a 1 GiB
block reports 196% of what dual-channel DDR5-6000 can physically carry, and at
256 MiB it reports 241%. A buffer that is never written maps every page to the
shared zero page, so the threads read one cached page and the aggregate scales
past the bus.

Publishing a figure that exceeds the bus is the exact defect this split exists
to remove, so threading was rejected. `mem.*` measures what one core can pull:
an understatement on a machine with more bandwidth than one core can use, but
always a floor, never an impossibility. Measured single threaded:

| | `cache.read` | `mem.read` | DDR5-6000 peak |
| --- | --- | --- | --- |
| archie (9800X3D) | 111.3 GB/s | 58.8 GB/s (61%) | 96 GB/s |
| macie (M5 Pro) | 36.3 GB/s | 35.5 GB/s | ~265 GB/s |

`cache.read` sitting at 116% of the DRAM peak on archie is the point rather than
a problem — it is cache, and it is `host`-scoped, so it claims nothing about any
other machine. macie shows no cliff at all, which is itself the finding: that
core is issue-bound rather than bandwidth-bound.

Because the measurement changed twice, `method` is at `mem.bandwidth/3.0.0` and
`compare` refuses to hold either older series against it.

### Conditions and gating

Battery, governor, load average, idle temperature, free memory, free disk and
thermal-throttle state are captured at the start of every run and stored with it.
A run is graded `clean`, `noisy` or `aborted`, and `compare` and `trend` use
clean runs only. Running on battery, under load, already throttling, or with a
nearly full disk refuses to measure unless `--force` is passed — an ungated
laptop series is pure noise, since macOS drops sustained power sharply on
battery.

### Tiers and SSD wear

| tier | contents | written |
| --- | --- | --- |
| `quick` (default) | cpu, memory, gpu, workload — read-only | 0 |
| `standard` | adds `fio` at `size=1g` and a 60 s sustained load | 12 GiB, capped at 30 |
| `heavy` | adds `fio` at `size=8g` and a 120 s sustained load | 58 GiB, capped at 70 |

The `fio` job starts from the Phoronix Test Suite's tuned parameters rather than
invented ones: `direct=1`, `runtime=20`, `ramp_time=5`, `iodepth=64`, `stonewall`
between stages and, on Linux, `disk_util=0` — which fio on Darwin rejects
outright, taking the whole job file with it, so it is omitted there.

**Read stages are time based; write stages are size based.** That asymmetry is
deliberate. Reads cost no endurance, so they run for a fixed `runtime` and the
volume does not matter. Writes do, so each write stage carries an explicit
`io_size` from `FIO_WRITE_SIZE` and the cost of a run is known before it starts
rather than discovered afterwards.

Under `time_based` writes the cost was whatever the drive could absorb in 25
seconds a stage — measured at 11.5 GB/s on an Apple NVMe, which is roughly 275
GiB for one `standard` run against a nominal 30 GiB cap. `WRITE_BUDGET` is now
enforced: `runner.execute` accumulates each job's predicted writes and refuses
any job that would cross the tier's budget, recording the refusal as a job
failure so the run grades `noisy` rather than silently measuring less.

Note that fio's own `io_bytes` excludes the ramp period, so it under-reports by
roughly the ramp's share; the recorded `written:` total is the larger of fio's
figure and the predicted bound. `store.total_bytes_written()` sums the stored
totals on demand — nothing calls it automatically.

That is why disk tiers are opt-in, why `quick` is the default and writes nothing
at all, and why none of this is wired to a timer. The layout files are removed on
every exit path — success, a parse failure, the 900 s timeout and Ctrl-C alike —
so the work directory does not accumulate gigabytes.

The work directory defaults to `~/.cache/dotfile/bench/work`, **not** `/tmp`,
which is tmpfs on this workstation — benchmarking it would measure RAM and report
it as disk. The filesystem actually measured is recorded in every run, and a
`--workdir` that lands on `tmpfs`, `ramfs`, `devtmpfs` or `hugetlbfs` is now
refused rather than merely recorded.

`direct=1` is not the safety net it looks like here. It is advisory on macOS,
where it maps to `F_NOCACHE`: the same job file run against a RAM-backed APFS
volume reported 18 024 MB/s as `disk.seq_read` without complaint. The fstype
gate catches the obvious case; a RAM block device carrying a real filesystem, a
network mount, or simply the wrong physical drive will still be measured as
whatever it is, which is why the filesystem and source are recorded.

### workload metrics

`workload.*` measures what is actually felt — nvim startup, `git status`,
`git log`, a `tar` of the source tree — through `hyperfine`, and records the
`dotfiles_sha` alongside. That makes a regression attributable to a config change
rather than to the hardware, which is why these live in their own namespace and
never feed the hardware baseline.

Each workload is preflighted once with a 20 second budget and dropped if it does
not finish. A cold `nvim --headless` can trigger a plugin manager sync that takes
many minutes; without the preflight that hang lands inside the measurement.

### native metrics

`cpu.native_*` and `mem.native_*` come from `bench-workloads`, the Rust binary
in `scripts/rust`. The other suites measure through whatever the platform
packages — a distribution's 7z and openssl are built with different compilers
and flags on every OS, which is exactly the variable a cross-machine rating
should not contain. `bench-workloads` is compiled from this repository with
codegen pinned in the workspace profile, has no dependencies, and does the
measured work outside any interpreter, so the method is identical on every
host and the tool version in each record is the crate version.

`suites/native.py` finds the binary at `scripts/rust/target/release/` (built
by setup when cargo is present), then `~/.local/bin`, and contributes nothing
when neither exists; `SYSINFO_BENCH_WORKLOADS` overrides the path for tests.
`cpu.native_*` runs a fixed data-dependent xorshift chain per thread, and
`mem.native_*` is single threaded for the same zero-page reason as `mem.*`,
with the buffer pattern-filled before timing starts.

### Health integration

`benchmark_issues()` returns the same `HealthIssue` tuples `health.py` produces,
so `sysinfo -hh` reports a drifting machine with no new rendering code:

```
Warning: disk.seq_write is 34% below its baseline
  2 410 MB/s at the baseline of 2026-08-01, 1 580 MB/s on 2026-08-13
  Re-run sysinfo bench run --only disk to confirm, then look for thermal or
  configuration causes
```

The direction follows the metric rather than the sign: throughput that fell
reads *below* its baseline, while a latency that rose reads *above* it. Saying
"below" for both meant a 20 ms → 30 ms startup regression was reported as
"50% below", which is the opposite of what happened.

It is composed at the CLI rather than inside `health_issues()`, so the snapshot
health function stays pure and reads no files. It is silent when no baseline
exists or the change is within the noise band. `dotfile check` grows a
`benchmark` row that reports a stale series.

---

## dotfile

Symlinks the configs tracked in this repo into `$HOME`, and maintains the
package manifest.

### The linking model

A *group* is a top-level directory of packages (`shared`, `linux/kde`, ...). A
*package* is one directory inside a group. `environment/<profile>/manifest`
lists the groups a machine links.

Each package is linked as a whole directory symlink when possible, because one
symlink is cheaper to reason about than a tree of them. That folding is skipped
in two cases:

- **A `config/targets.dotfile` entry points inside the package.** The entry has to be able to
  land somewhere else, so the parent must be a real directory.
- **The destination is a directory that must never itself become a symlink** —
  `$HOME`, `~/.config`, `~/.local`, `~/.local/share`, `~/.local/bin`,
  `~/.config/systemd`, `~/.config/systemd/user`. Replacing any of these with a
  symlink into the repo would capture every unrelated file the system later
  writes there.

When a directory that was previously folded stops qualifying, it is *unfolded*:
the directory symlink is replaced by a real directory containing one symlink per
entry. This is why `link` is safe to re-run after adding a `config/targets.dotfile` entry.

Groups are linked in manifest order, and a later group may hold a package of the
same name as an earlier one. The shared copy is linked first, then unfolded file
by file, and each file the later group also carries replaces its link. So a
platform group overrides individual files of a shared package while inheriting
the rest — how `shared/fastfetch` is specialised per platform.

### targets.dotfile

`config/targets.dotfile` maps a repo path to a destination. Without an entry, a package lands
at `~/.config/<package>`. Matching is longest-prefix, so a specific file entry
beats the package entry containing it.

### .nolink

A package containing a `.nolink` file is skipped entirely. Used for configs that
are tracked for reference but must not be installed on this machine.

### remove

`dotfile remove <path>` stops tracking exactly the requested repository path and
keeps its contents at the live destination. A package path removes the package;
a path inside a package removes only that file or directory. A leading slash is
treated as the root of the dotfiles repository. When a real live path already
exists, it is kept unchanged instead of being overwritten by the tracked copy.

### Conflicts

An existing file that is not a symlink into this repo is never touched. It is
collected and reported at the end, and `link` exits non-zero. Foreign symlinks
are treated the same way. The user resolves each one by hand — silently moving
real configs aside is how people lose data.

### Pruning

Before linking, symlinks under `$HOME`, `~/.config` and `~/.local` that point
into the repo are removed when they are broken, or when they point at an
override set that is no longer selected. The second case matters because a
stale override link is not broken — it points at a file that still exists — so
a plain dangling-link check would leave it behind and the machine would keep
using the wrong override.

### Overrides

A group may contain `overrides/<name>/` directories holding machine-specific
variants. The selection is per group, stored in `~/.config/dotfile/overrides`,
and persists across runs. `--override <group>=none` opts out. When a group has
overrides and none is selected, `link` prints a note rather than guessing.

### check

`link` reports what it did and `status` only looks at symlinks, so neither
notices a profile linked onto a machine where the programs, fonts and plugins
those configs need were never installed. That is the whole subject of `check`:
symlink state stays in `status`, which already reports it destination by
destination. One row per subject, the misses listed underneath it:

```
  check  macos

  ✗ tools      2 missing
      yazi
      rg    ripgrep
  ✗ fonts      1 missing
      Noto Sans
  ✗ plugins    1 missing
      zsh-autosuggestions
  ✓ brewfile   5 installed
```

`✓` and `·` pass, `✗` and `!` fail. The rows are:

- **profile, commands, overrides, shell** — the profile belongs to this platform
  (only judged when `environment/<platform>/` exists, so an unrelated profile
  name is not guessed at), every declared command is installed through uv into
  `~/.local/bin` and resolves there, every group with `overrides/` has a
  selection, and the login shell is zsh.
- **tools, fonts, files, optional** — the entries in `config/requirements.dotfile` for the
  groups this profile links. Fonts are matched against `fc-list` families, or
  against the font directories when fontconfig is not installed; a family
  matches its own weights (`HackNerdFontMono-BoldItalic`) but not a longer name
  (`Noto Sans` is not `NotoSansAdlam`).
- **plugins** — the zsh plugins the linked configs name, looked for in every
  directory those configs load from: `$ZSH/custom/plugins`, the `/usr/share`
  paths the Linux fragments use, and Homebrew's `share`.
- **package lists** — `macos/Brewfile` through `brew list` when the profile
  links `macos`, `pkglist.txt` and `aurlist.txt` through `pacman -Qq`. A list
  whose package manager is missing is skipped rather than reported as hundreds
  of missing packages.

Lists stop at twelve entries; `--all` prints every one.

### config/requirements.dotfile

Hand-maintained, one block per group, in the same grammar as
`config/packages.dotfile`:

```
shared {
  git                       a command that has to be on PATH
  nvim = neovim             ... installed under a different package name
  ?docker                   wanted but not required: reported, never a failure
  font Hack Nerd Font Mono  a font family the configs ask for by name
  file ~/.config/hypr/wallpaper.png    a path that has to exist, tracked elsewhere
}
```

Keyed by group rather than by profile, so each requirement is stated once and a
profile picks up exactly the groups its manifest links. A group that is not a
directory in the repository is an error, so a typo cannot silently check
nothing. Font entries carry no package name because it differs too much between
Homebrew and pacman to be worth printing; the family name is what you search
for either way.

Requirements are declared rather than derived from the package directories,
because the two do not line up in either direction: `shared/zsh` needs `fzf`,
`eza` and `bat` that own no config here, and `shared/skills` needs no program
at all. Zsh plugins are the exception — they are read out of the linked configs
(`$ZSH/custom/plugins/<name>`, and any `<dir>/<name>/<name>.zsh` a fragment
sources), so the list cannot drift from what the shell actually loads.

### secret

`dotfile secret scan` looks for three things at once: token and key shapes,
literal private values read from `~/.config/dotfile/canaries`, and structural
rules about encrypted files. It scans tracked files by default, `--staged` for
what a commit is about to record, and `--commits <range>` for every blob a set
of commits adds. `--no-canaries` drops the middle tier for anything that must
not hold the value list.

The pattern set is shared with the transcript archiver rather than duplicated,
so `redact()` and the scanner cannot disagree about what a secret looks like.
Matches are printed masked and canaries are printed by label only, because the
output lands in scrollback and in transcripts.

`pre-commit` runs it last, after the steps that stage generated files, so
nothing reaches a commit unscanned. `pre-push` runs it over the commits being
pushed, which still catches a value that a `--no-verify` commit slipped in and
a later commit removed. Both fail closed when `scripts/python/.venv` is missing.
Allowed false positives live in `config/scan.dotfile`; canary and invariant findings
cannot be allowed.

`init` writes this machine's identity, `enroll` and `revoke` edit
`config/keys.dotfile`, `sync` regenerates `.sops.yaml` from it (`--rewrap` also runs
`sops updatekeys` over every encrypted file), `keys` lists what is enrolled,
and `doctor` checks the whole chain from binaries through hooks.
`config/keys.dotfile` is the source of truth and `.sops.yaml` is generated and staged
by `pre-commit`, the same relationship `config/packages.dotfile` has with
`PACKAGES.md`.

`add`, `edit`, `apply`, `status` and `clean` work the vault. Encrypted material
is written to its destination as a real file rather than symlinked, so that no
plaintext ever exists inside the repository. A package carrying a `.secret`
marker is materialised whole and may contain nothing unencrypted; a lone
`*.enc` file inside an ordinary package is materialised on its own while the
rest of that package links normally.

Because destinations are copies rather than symlinks, an edit made directly to
one is real work that the repository does not know about. Those are reported as
`drifted` and never overwritten: `apply`, `clean` and `link` all refuse and
exit non-zero. `edit` is the supported way to change a secret, and
`apply --force` is the way to throw the local edit away.

A `*.tmpl` file renders to the name without the suffix, substituting
`{{ dotted.name }}` from `vars.enc.yaml`, which is encrypted structurally so
its keys stay readable in a diff and only the values are ciphertext. Templates
follow the same path as decrypted files: same drift rule, same 0600, never
symlinked, never written back into the repository. A template naming a var
that does not exist is `unresolved` and nothing is written, because a half
rendered config is worse than none. `secret vars` lists the names and their
consumers, never the values.

Every declared value also becomes a canary for the scanner and a redaction for
the transcript archiver, labelled by its dotted name, so a value in
`vars.enc.yaml` cannot be committed in plaintext from the moment it exists.
Names under a top level `open:` key are exempt, for per-machine values that are
not private.

`link` runs `apply` as its final phase. A machine with no age identity reports
its secrets as `sealed` and links everything else, so an unenrolled machine is
still usable.

See `docs/secrets.md` for the threat model behind all of it.

### system

`dotfile system` tracks root-owned files — `/etc` and the like — that the
linker deliberately will not touch. A package carrying a `.system` marker is
never symlinked and never installed by `link`; it mirrors the destination tree
inside itself, so `linux/arch/macie-usb/etc/systemd/network/x.link` installs to
`/etc/systemd/network/x.link` under a single `config/targets.dotfile` line pointing the
package's `etc` at `/etc`.

Symlinking is not merely unsupported here, it is unsafe. systemd, dnsmasq and
NetworkManager read those files as root; a symlink into a user-writable
repository turns write access to `$HOME` into root. So these are copied with
`install -o root -g root`, never linked, and `add` refuses any source under
`$HOME` for the same reason the linker refuses sources outside it.

Templates and vars work exactly as they do in the vault, which is the point: a
`.link` file matching on a MAC address becomes a `.tmpl` naming a var, and the
address itself lives encrypted in `vars.enc.yaml` rather than in a public
repository.

`status` and `diff` are read-only and need no privileges, since the files are
world readable. `install` is the only command that touches the system, is never
run by `link`, asks before writing unless given `--yes`, and refuses outright
while any file is unresolved — a half-installed network configuration is worse
than an out-of-date one. Destinations must be absolute, must not be under
`$HOME`, and their top-level directory must already exist, so a typo in
`config/targets.dotfile` cannot invent a path.

Modes are `0644`, `0755` when the tracked file is executable, and `0600` for
anything encrypted. After installing, the command prints the reload each
destination needs — `daemon-reload` for units, `udevadm` for `.link` files,
a NetworkManager reload for its drop-ins — rather than running them, because
those interrupt a working network.

### Profiles

The active profile is stored in `~/.config/dotfile/profile` and reused when
`link` is called with no argument.

Profile names were renamed once (`desktop/arch-linux/kde` -> `arch-linux/kde`
and friends). The compatibility shim that translated the old names has been
removed. A machine whose saved profile predates the rename will report
"no manifest for profile" and list the available ones; pass the new name once
and it is saved.

### format

Normalises tracked `.conf` files. Three modes, chosen by path:

- **hypr** — reindents blocks four spaces per level and normalises `key = value`
  spacing. A `}` that closes a block absorbs a preceding blank line.
- **kitty** — aligns values into a column, and aligns `map` bindings into a
  second column keyed on the shortcut, so keybindings stay readable as a table.
  Requires buffering the whole file to measure the widest key first.
- **plain** — collapses runs of blank lines and strips trailing whitespace.

Generated colour and font files (`colors*.conf`, `kitty/conf.d/fonts.conf`) are formatted as plain so the
generator's own column alignment survives.

---

## dotfile theme

A *theme profile* is a colour palette plus fonts. `config/profiles.dotfile` says which
profile each group uses, and this stamps them into every config that carries
colour or a font.

```
dotfile theme                         the menu: switch, status, show, apply, check
dotfile theme switch [profile] [scope]  assign a profile, then restamp
dotfile theme status                  which profile each group uses, and what has drifted
dotfile theme show [profile]          palette, roles, fonts and a sample of the terminal
dotfile theme apply                   regenerate from config/profiles.dotfile
dotfile theme check                   report what would change, exit 1 if anything would
dotfile theme outputs                 every file the generator owns
dotfile theme outputs --stageable     only the ones safe to auto-stage
```

This was the standalone `generate-theme` command. It moved under `dotfile`
because it is the same job as the rest of that tool — reconciling the repo with
the machine — and because the picker needed somewhere to live.

### Switching

`switch` is the only thing here that writes `config/profiles.dotfile`. With no
arguments it asks what should change first (everything, one group, or one
package inside a group), then which profile, drawing each candidate as a card
painted in its own colours: the palette, a prompt, a file listing, and the
selection, accent and tab chips. Choosing restamps every file the assignment
covers.

A *scope* is `everything`, a group (`linux/kde`), or a package inside one
(`shared/obsidian`); anything else is an error naming the groups that exist,
rather than writing a key that resolves to nothing. `everything` is the only
scope that removes assignments — it drops every group and package key so the
one `shared` fallback is left, and says which ones it dropped first.

Value edits keep the line they are on, comment and alignment included. Adding a
key realigns the block it joins; removing the last key in a block takes the
block with it.

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

`theme` sets the group's profile; any other key names one package inside that
group. Resolution runs package -> group -> `shared`'s `theme`, which is the
fallback every unlisted group lands on, so the minimum useful file is one
block. `switch` writes it, but editing it by hand is still switching: the path
unit regenerates on save either way.

Selection is per *group* because that is the granularity that physically
exists — every generated file belongs to exactly one group, so a group key
resolves to an exact set of files:

| group | owns |
|---|---|
| `shared` | kitty, wezterm, starship, zsh, obsidian, nvim, fastfetch |
| `linux/common` | GTK colours and settings, quicklaunch |
| `linux/kde` | kdeglobals, desktop-appletsrc, konsole, panel presets |
| `linux/arch`, `linux/ubuntu`, `macos` | that platform's fastfetch config and logo |

This is what makes light Plasma against dark terminals a one-line change.

A package key only works where that group already owns the file, so
`linux/arch { zsh = latte }` is an error naming what `linux/arch` does own
rather than a silent no-op — `shared/zsh/conf.d/03-theme.zsh` belongs to
`shared`. Making it work would mean generating an override into
`linux/arch/zsh/` and leaning on the linker's later-group-wins rule. That is a
real idiom here (fastfetch uses it), but it is only safe for fully generated
files; for the marker-edited ones it would mean copying a hand-maintained file
into a second group where it would drift, so it is deliberately not built.

### Why a profile is stamped rather than switched at runtime

kdeglobals, the GTK stylesheets, the Obsidian theme and `starship.toml` can
each hold one scheme. Emitting every profile side by side would only work for
the terminals, so the choice is resolved at generate time instead. Every output
is tracked, so the repo encodes one assignment and changing it is an ordinary
commit. Two machines cannot run different assignments from the same checkout —
that is the price of the outputs being tracked at all.

### Colour indirection

Four layers now. `[palette]` in the profile holds named colours (`mauve`,
`base`). `theme/roles.toml` maps a semantic name to a palette name
(`prompt_git = "green"`) and holds `[terminal]`, `[eza]`, `[kde]` and
`[konsole]` the same way. Configs reference roles, so recolouring means editing
one role rather than hunting hex values.

`roles.toml` is shared because a well-named palette makes it portable: swap
every colour for its light counterpart and `prompt_git = "green"` still means
the right thing. Where that breaks, a profile overrides the individual key —
`theme/profiles/latte.toml` restates four `[terminal.ansi]` slots because the
shared mapping picks the greys for a dark background, and on a light `base`
that inverts.

Overrides deep-merge key by key, so a profile states only what differs. A
profile may override any table in `roles.toml` or `fonts.toml`.

### Adding a profile

One file, `theme/profiles/<name>.toml`, with `name`, `dark`, `icons`,
`[nvim] flavour` and a `[palette]` holding every colour name the shared layers
reference. `dotfile theme` validates that up front and reports everything
missing at once, rather than dying on the first unknown name partway through
the emitter list. It also rejects two palette entries sharing a hex, and a
`[kde]` role shadowing a palette name — both silently corrupt the retagging
described below.

### Fonts

`theme/fonts.toml` carries the families (`general`, `nerd`), the sizes
(`terminal`, `terminal_mac`, `interface`) and the per-application opt-in. It is
shared rather than per-profile so `[applications] obsidian` cannot drift
between profiles and silently switch Obsidian's font block off.

The sizes exist because the generator now owns font settings that used to be
hand-maintained copies of the theme: `shared/kitty/conf.d/fonts.conf`, the
`Font=` line in the Konsole profile, `gtk-font-name` in both GTK
`settings.ini`, and the `sizes` table wezterm reads. Konsole's `Font=` is a
`QFont::toString()` value, so only the family and point-size fields are
replaced and the rest of the record is left as Qt wrote it.

### What `dark` drives

`dark` is not decoration. It selects `color-scheme` in the Obsidian theme and
`gtk-application-prefer-dark-theme` in both GTK `settings.ini`; `icons` picks
the matching icon theme, which is not derivable from the colours. Without
these a light profile would render dark chrome around light content.

### In-place edits vs generated files

Files that are fully generated carry a "do not edit" header. Files that are
partly hand-maintained are edited between `theme:<name>` and `theme:<name>:end`
markers, or by KConfig section, so the rest stays hand-editable.

### Why kdeglobals and desktop-appletsrc are not auto-staged

Both are regenerated, but a running plasmashell rewrites them continuously with
unrelated widget state. Auto-staging them would sweep that churn into every
theme commit. They are staged by hand, after restarting plasmashell. This is
expressed in the emitter registry as `stageable = False`, and the pre-commit
hook stages exactly the emitters that declare themselves stage-safe.

### The Obsidian theme

`shared/obsidian` is a normal package linked to `~/Documents/main/.obsidian`.
`themes/Fredrir/theme.css` is a hand-editable stylesheet; the generator replaces
only the block between the `theme:variables` markers, the same way it stamps the
palette into `starship.toml` and the fastfetch config.

```
theme/profiles/<name>.toml the colours
theme/maps/obsidian.toml   which colour each CSS custom property takes
        -> the theme:variables block inside shared/obsidian/themes/Fredrir/theme.css
```

Everything outside those markers is authored by hand: radii, spacing,
transitions, selectors. Rules that need a colour reference the theme's own
custom properties (`var(--interactive-accent)`, `var(--color-blue-rgb)`) rather
than naming a palette colour, so the whole file stays valid CSS with no
placeholder syntax and no build step to read it.

`manifest.json` beside it is a plain tracked file. Obsidian needs it to load the
theme, but its contents are static and have nothing to do with the palette.

`[variables]` is a single ordered table rather than separate colour and alpha
sections, because the entries are interleaved and CSS output order follows the
table. Four value forms:

```toml
"--color-base-00"             = "crust"
"--color-red-rgb"             = { rgb = "red" }
"--background-modifier-cover" = { color = "crust", alpha = "0.72" }
"--accent-h"                  = { derived = "mauve_h" }
"--scrollbar-bg"              = { literal = "transparent" }
```

`rgb` emits the `r, g, b` triple Obsidian expects for its `-rgb` properties.
`derived` reads a value computed from the palette rather than a palette entry —
the accent hue, saturation and lightness are converted from `mauve` via HLS.
`alpha` is stored as a string so the rendered decimal is exactly what was
written, with no float formatting in between.

Structural CSS outside that block (callout tints, `::selection`, tab shadows)
reaches colour only through the custom properties the block defines, so it
needs no substitution of its own.

`color-scheme` is emitted as the first line of the block rather than written by
hand above it, because it has to follow the profile's `dark` flag.

### fastfetch per platform

fastfetch reads one config, `~/.config/fastfetch/config.jsonc`, so the layout
cannot branch on the host at runtime. The branch happens at link time instead:
each styled platform owns a group holding its own `config.jsonc` and logo, and
because that group is linked after `shared`, its config replaces the shared one.

```
shared/fastfetch/config.jsonc         the fallback: no logo of its own
linux/arch/fastfetch/{arch,config}    󰣇  arch.txt
linux/ubuntu/fastfetch/{ubuntu,config} 󰕈  ubuntu.txt, no DESKTOP section
macos/fastfetch/{apple,config}        󰀵  apple.txt
```

The branding lives in the platform groups rather than in `shared`, `linux/common`
or `linux/server`, because those are linked by machines the logo would be wrong
for: `shared` reaches every host, `linux/common` would put an Arch logo on any
Linux desktop added later, and `linux/server` would put an Ubuntu one on any VPS.
An unstyled platform therefore links only `shared`, whose logo is
`{"type": "builtin"}` — fastfetch draws the distro it detects, in its own
colours, rather than a logo we picked for a different machine.

The configs differ only where the platform forces it: the OS and kernel key
icons, `hideType` on GPU (an Apple Silicon GPU reports as integrated, so hiding
integrated GPUs there hides the only one), the disk folders, and modules that
detect nothing on that platform — `de` on macOS, the whole desktop section on a
headless server. Everything else is deliberately identical.

### fastfetch logo gradient

Every logo in `FASTFETCH_LOGOS` is recoloured with a linear gradient
interpolated across the four section accent colours, one step per line of ASCII
art. The logos have different line counts, so the gradient is spread over each
file's own height and all of them end on the same four stops. Existing escape
codes are stripped before recolouring so the operation is idempotent.

The same applies to the configs in `FASTFETCH_CONFIGS`: each carries the
`theme:constants` markers and all are stamped with the same palette, so no
platform's config drifts from the others.

### eza colours

`EZA_COLORS` matches by `*.ext`, not by file type, so each category in
`[eza.categories]` is expanded into one glob per extension. Categories are
emitted first and explicit `*.ext` entries last, so an explicit entry wins.
`LS_COLORS` is unset because eza prefers it when both are set.

### Retagging Catppuccin colours in KDE widget config

panel-colorizer presets and `desktop-appletsrc` embed literal hex and `r,g,b`
colours written by the widgets themselves. Only values that exactly match a
colour the generator recognises are rewritten. Anything else is left alone,
because widget placeholders and gradient defaults share the same syntax and
rewriting them would corrupt unrelated settings.

The recognised set is every profile's palette, plus the upstream Catppuccin
hexes in `maps/catppuccin.toml`. Every profile matters, not just the active
one: after switching to latte these files hold latte literals, and switching
back has to recognise them to undo it. Restricting the set to one profile would
make each switch lossy and the files would decay. Two profiles may not give the
same hex two different role names — the generator refuses to run rather than
guess which one wrote a literal.

The cost is that the set of hexes a human can hardcode in those files shrinks
with every profile added. Eight-digit hex is never rewritten, because
`#RRGGBBAA` and `#AARRGGBB` are not distinguishable here.

---

## count and path

Two one-shot commands that run inside prompts, loops and keybindings, where
the interpreter start was most of the wall clock. They are Rust binaries in
`scripts/rust`, sharing the `workstation` crate for what every tool in that
workspace agrees on: a `--completions <shell>` flag shaped the way
`shared/zsh/conf.d/55-completions.zsh` expects, a failure reported as
`program: message` on stderr with a non-zero status, and — for the ones that
draw something — the palette `dotfile theme` exports, the terminal's width,
and the question asked before anything irreversible.

`count` counts a directory's entries; `-r` counts everything underneath it
instead, and `-d` leaves hidden entries out. Under `-r` the two flags agree on
what hidden means: a hidden directory takes its whole subtree with it, so a
subtree is either counted whole or skipped whole. A symlinked directory counts
as one entry and is not descended into, which is what keeps a link loop from
becoming a hang. Sub-directories are read in parallel, and a directory that
cannot be read is reported on stderr rather than passed off as empty.

`path` prints where a target sits: relative to its repository root as
`/sub/file`, relative to the home directory as `~/sub/file`, or absolute when
it is outside both. The root is the nearest ancestor holding a `.git` entry —
a directory in a plain clone, a file in a worktree or a submodule — found by
walking up rather than by asking `git rev-parse --show-toplevel`, because the
spawn was the whole cost. The two answers agree except inside `.git` itself,
where git declines to answer and this prints `/.git/...`. Targets need not
exist: the part that does is resolved through symlinks and the rest is
appended, so a file that is about to be written still describes itself.

---

## flatten

`flatten <dir>` undoes redundant nesting and nothing else: while the directory
holds exactly one entry and that entry is a directory, the wrapper is emptied
into the target and removed, over and over until the target holds something
other than a lone directory. The archive that unpacked one folder too deep is
the right shape afterwards. Two entries can never land on one name that way,
so it does not ask, and it prints nothing — running it twice is running it
once.

`-d` means the whole subtree instead: every entry that is not a directory
comes up to the top, and every directory underneath goes away. That can put
two entries on one name and it removes directories, so it follows the `gdd`
shape — print the plan, ask, then act. Each contested name is asked about on
its own with `[Y/n/a]`, where `a` answers the rest of the run; the shallowest
entry holds a name by default, and an entry already in the target holds its
name without going anywhere. There is no `--force` and no automatic renaming,
because both of those decide on somebody's behalf what only they can decide.
A deep flatten refuses `/` and the home directory outright.

The two conflicts nobody is asked about are settled by the plan. A name that
is both an entry coming up and a directory in the target is fine when the
directory is on its way out — it is moved aside first, under a hidden name,
and removed at the end. It is not fine when something inside that directory is
staying, and then the move is the thing given up, said out loud on stderr.

Everything is settled before the first rename, which is what makes `-n` a true
account of the run it stands in for rather than a guess at it. The survey
reads each directory's subdirectories in parallel, the way `count` does, and
uses the directory entry's own type so a symlink is never mistaken for the
directory on the other end — a link comes up as itself, and a loop is not a
hang.

The moves are `renameat` between two open descriptors rather than renames of
two paths. A name is resolved once, against a directory the kernel already
holds, instead of once per path component per file, which on a deep tree is
the difference between one lookup per file and one per level per file. Each
step down is opened with `O_NOFOLLOW`, so a symlink swapped in underneath a
running flatten cannot redirect it out of the tree. And a descriptor goes on
naming its directory after the directory is renamed, which is what lets a
collapse move a wrapper out of the way and still lift entries up through it.

---

## gdd and gpp

The two git commands live in `scripts/rust/crates/git` and share `gitkit`,
which is the only crate that talks to gitoxide: it opens the repository,
surveys the working tree, renders the plan, and carries it out. Each binary is
the command-line shape around that.

`gdd` discards every change: tracked files go back to `HEAD`, untracked files
are deleted. It prints what that means before doing it, because half of it
cannot be taken back — a restored file is still in `HEAD`, a deleted untracked
file is nowhere — and then asks, unless `-y` says not to; `-n` stops after the
plan. Ignored files are neither listed nor touched, and a repository nested in
the working tree is listed under `kept`, because `git clean` refuses to delete
one and the plan should not promise what will not happen. Paths limit the run
and are read as pathspecs relative to the current directory, the way git reads
them.

The installed binary is named `git-discard`, while `gdd` is a shell alias.
That keeps the short command without colliding with the `gdd` name used for
GNU dd on macOS (which `fzf-tab` probes when completing commands).

An untracked directory is one row and one deletion when everything inside it
is untracked too, since the whole thing goes; one that holds anything ignored
stays open instead and its untracked files are listed individually, because
the row is what gets removed and a row for the directory would take the
ignored files with it. That is the same place `git clean -d` stops.

The plan is three sections, one per fate, with the destructive one in red.
Columns are measured over the rows that are actually shown, a section stops at
twelve rows unless `-a` asks for all of them, and the counts are the diff
against `HEAD` that would be thrown away: per row, and as a total on the
summary line that includes untracked files.

Behind it is one status walk — gitoxide runs the staged, the unstaged and the
untracked comparisons at the same time — after which every changed path is
looked up once in `HEAD`'s tree, which settles both how it is labelled and
whether discarding it is a restore or a deletion. The line counts are the
numbers `git diff --numstat` gives, computed through gitoxide's resource cache
so that `.gitattributes` filters and the binary-or-text judgement are the same
ones git would apply, and computed in parallel across the changed paths. The
shell version this replaced spent a whole `git diff` process per untracked file
on that number alone.

The discard is gitoxide too. Deletions go first, because a path on its way out
can be standing where a path on its way back belongs; a directory goes whole,
except for a repository inside it, which `git clean` steps around as well, and
the empty directories a removed file leaves behind go with it. Then the entries
to restore are checked out from `HEAD` — filters, executable bits and symlinks
included — and the index is rewritten once, with its cache-tree dropped, since
those cached subtree ids describe the entries that were just replaced and a
later `git commit` would otherwise believe them.

`gpp` is `git add :/` — everything, from the repository root, whichever
subdirectory it runs in — then `git commit -m <message>`, then `git push`. It
stops
at the first failure and exits with that step's own status, so git's vocabulary
— 128 for "not a repository" and so on — survives for whatever is chained after
it. Those three stay with git: they are the steps that run hooks, sign, and
reach the network with the user's credentials. The question between them is
answered here, because `git commit` with nothing staged prints a status report
and fails, which reads like a fault in the tool. git leaves the tree its index
would write in the index itself, so the answer is usually one comparison of two
hashes; only a missing or stale cache-tree falls back to comparing the whole
tree against `HEAD`. Message words are joined with spaces, and a word may start
with `-`. `gff` is a shell alias for `gpp .`.

---

## dmux

One CLI for every session on either machine: wezterm-mux workspaces and tmux
sessions, local or on the peer, from `scripts/rust/crates/dmux`. It replaces
the internals of the old ssa/ssm shell functions —
`shared/zsh/conf.d/91-tmux-attach.zsh` now only wraps it: `ssa` forwards to
`dmux --host archie` and `ssm` to `dmux --host macie`, a bare session name
becomes `con -A` (the old create-or-attach default), and the remaining old
flags (`--tmux`, `-l`) intentionally error rather than silently change
meaning. `dmx` is
the shell alias, and all three borrow dmux's completion.
`-H/--host <macie|archie>` points any invocation at the peer.

The verbs: `ls` (alias `list`) merges wezterm workspaces and tmux sessions
into one indexed list, `--json` for machines — rows carry a `host` field,
and indices are assigned over the merged set before `--tmux`/`--wez` filter,
so a filtered listing has gaps but its numbers still name what `con` and
`rm` resolve. `con <name|index>` attaches an existing session and refuses to
invent one, so a typo cannot leave a stray session behind; `-A/--create`
falls back to what `new` does. `new <name>` creates then attaches (tmux `-A`
semantics), with `--dir <path>` for the working directory and a command
after `--`. `rm` (aliases `kill`, `delete`) kills after asking — the [y/N]
goes to stderr, and a non-TTY without `--yes` is refused rather than
answered silently; `rm --all` sweeps every tmux session but keeps the one
the client is sitting in, with a note saying how to kill it explicitly.
`detach` hands the client back and leaves the session running; `rename`
renames; `dmux -` toggles back to the previously attached session, tracked
per host, so `ssa -` toggles on the peer; `keys` prints the live wezterm and
tmux key tables instead of a hand-maintained copy (`--man` renders them as a
man page, via a randomly named temp file); `doctor` reports what the
transport probes see, `--json` included. Bare `dmux` on a TTY is a picker,
and a bare name is treated as `con` — `dmux myproj -w 2` works, the one flag
the fallthrough shares with `con`. Inside wezterm the picker and `con` can
also switch to a wezterm workspace, by activating one of its panes.

Attaching a remote host walks the chain the shell version had, now in one
place. Inside wezterm a bare attach is a native mux tab on the peer's ssh
domain — the `-usb` domain when the cable answers the probe, the `-ts`
(Tailscale) domain when it does not. Anything else — outside wezterm, or any
named session, since tmux sessions are a tmux concept — is
`ssh -t <host> exec tmux new-session -A -s <session>`, and each step down
the chain is one stderr line saying why. Remote arguments are quoted for the
peer's zsh — a session named `={a,b}` or `$(reboot)` reaches tmux as text —
and `wezterm cli` probes run with `--no-auto-start`, because a listing is a
question, not a request to boot a mux server. Attach replaces the process with
`exec` so the TTY is handed over cleanly, which is also why it cannot be
observed from a test: `DMUX_DRY_RUN=1` prints the command that would have
been exec'd instead of running it, and that is how the integration tests —
and a doubtful user — inspect transport selection without losing the
terminal.

---

## clean-copy

Rewrites the clipboard as clean plain text: CRLF to LF, ANSI/OSC escapes
stripped, non-breaking and zero-width spaces normalised, stray control
characters dropped (tabs kept), trailing whitespace removed, leading and
trailing blank lines trimmed, and the longest whitespace prefix common to
every non-blank line removed, so relative nesting survives while the
terminal-UI indentation that tools like codex leave behind does not. Because
the cleaned text is re-copied, only `text/plain` is offered afterwards, which
is what strips rich-text formatting.

Kitty passes its current selection directly to `clean-copy --stdin`, avoiding
a clipboard readback and timing delay. Konsole uses the binding in
`linux/common/xremap/config.yml`: xremap re-emits the native copy binding,
waits for the clipboard update, then launches `clean-copy`. Re-emitting the
same combination cannot loop because xremap never watches its own virtual
output device. If the venv is missing, Konsole still copies and cleanup fails
silently.

A non-text clipboard (an image), invalid UTF-8, or whitespace-only content is
left untouched.

## transcript

`transcript` archives Claude Code and Codex sessions as monthly Obsidian notes.
Projects can share a transcript group through `[groups]`. A group normally lives
at `Transcripts/<group>`, but `[destinations]` can replace that root with a path
relative to the configured vault:

```toml
[groups]
dotfiles = ["dotfiles"]

[destinations]
dotfiles = "Dotfiles/Agents"
```

With that configuration, a July Codex session is written to
`Dotfiles/Agents/2026-07/codex/<note>.md`. Groups without an override retain the
normal `Transcripts/<group>` layout. `transcript migrate` previews file counts
grouped by directory, while `--verbose` lists every relative file path. It
refuses destination conflicts and asks for confirmation with `[y/N]` before
changing anything.

## cpa, cpas and acp

Transfer the plain-text clipboard between macOS and the KDE Wayland session on
Archie over SSH:

```bash
cpa                 # macOS clipboard -> Archie
cpa --sensitive     # same, with the sensitive clipboard hint
cpas                # shorthand for cpa --sensitive
acp                 # Archie clipboard -> macOS
```

The commands preserve the text bytes, including Unicode and trailing newlines,
and pass them only over SSH standard input or output. Clipboard contents never
become command-line or remote-shell arguments and are never printed. Empty,
whitespace-only, non-text and invalid UTF-8 clipboards fail without changing
the destination clipboard.

Non-interactive SSH sessions do not inherit Archie's graphical environment.
The commands therefore resolve `WAYLAND_DISPLAY` from the active user systemd
environment and set `XDG_RUNTIME_DIR` before launching `wl-copy` or `wl-paste`.
An active Wayland session and the `wl-clipboard` package are required.

`--sensitive` asks compatible clipboard managers not to retain the copy in
history. It is a hint rather than a guarantee; secrets should still be handled
as if the destination clipboard manager may retain them.

## update-readme-fastfetch

Regenerates the preview block between the `fastfetch:start` / `fastfetch:end`
markers in `README.md`. No-ops when fastfetch is not installed.

### Shell and Terminal are recomputed

fastfetch identifies the shell and terminal by walking the process tree. Run
from a git hook, that tree is `git` -> `bash` -> `fastfetch`, so it reports
"bash" and "git" instead of the real values. Both are recomputed — the shell
from the login shell in the passwd entry, the terminal from environment markers
— so the preview is correct however it was generated.

### Column alignment

fastfetch marks the value column with an `ESC[<n>G` cursor-move, which a
terminal resolves but plain text cannot. The script uses that escape only as a
split point and then realigns the columns itself.

Widths are measured in terminal cells, not characters. Nerd Font glyphs live in
the Unicode Private Use Areas and render double-width, and East Asian wide and
fullwidth characters do the same, so both count as two cells. Combining marks
count as zero. Getting this wrong shifts the whole value column.

The `Local IP` line is dropped so a local network address is not committed.
