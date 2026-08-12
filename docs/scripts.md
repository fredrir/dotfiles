# scripts

Reference for the workstation command-line tools in `scripts/`. Code files
carry no comments, so the reasoning behind the non-obvious behaviour lives
here.

`scripts/` is a uv-managed Python project (Typer + Rich, package name
`tools`). `uv sync --project scripts --locked` installs its development
environment into `scripts/.venv`. Setup also installs the project as an
editable uv tool and exposes only the console entry points declared in
`scripts/pyproject.toml` through `~/.local/bin`, with runtime dependencies
constrained by `scripts/uv.lock`. Setup enables install-time bytecode
compilation for both environments. Re-running setup reconciles that directory
with the declaration, including pruning removed entry points. The Linux,
macOS, and Ubuntu zsh profiles put `~/.local/bin` on PATH without exposing the
project environment's Python or dependency commands. The Ubuntu VPS bootstrap
uses `./setup.sh --commands-only` to install the same entry points without a
development environment.

## Layout

```
scripts/
  pyproject.toml             project, dependencies, console entry points
  uv.lock                    locked dependency versions
  src/tools/
    core/
      console.py             shared output, errors, color gating
      paths.py               repository root discovery, ~ shortening
      process.py             subprocess helpers
    utils/
      count.py               count items inside a directory
      size.py                du-backed size summary
      path.py                repo-relative or home-relative path of a target
      tardirs.py             tar archive directory tree with entry counts
      gpp.py                 git add + commit + push
      remote_clipboard.py    text clipboard transfer between macOS and Archie
      sysinfo/               modular hardware and software summary
        cli.py               pretty, full and health command flags
        collect.py           Fastfetch, process and NVIDIA probes
        models.py            shared snapshots, components and render options
        branding.py          vendor registry, marks, colours and fallbacks
        profiles.py          exact-model facts and operating limits
        devices.py           shared device normalization and telemetry helpers
        facts.py             shared normalized fact construction
        formatting.py        shared units, percentages and text formatting
        identity.py          platform-aware username and hostname resolution
        normalization.py     platform and device sanitation helpers
        typography.py        terminal-safe block lettering
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
      model.py               palette, roles, fonts, colour conversion
      render.py              file writing and in-place editing
      emitters.py            one function per generated config
      registry.py            emitter list and their declared outputs
      cli.py                 argument handling
    dotfile/
      cli.py                 command dispatch
      state.py               repo context, profiles, overrides, manifests
      targets.py             targets file -> destination mapping
      link.py                link / unlink / prune engine
      check.py               health of the installed environment
      packages.py            packages.dotfile and PACKAGES.md
      add.py                 adopt a live config into the repo
      remove.py              stop tracking a path, keep it live
      format.py              .conf formatter
      profiles.py            host platform and desktop detection
  tests/                     pytest suites per area
tests/
  run.sh                     black-box test runner
  lib.sh                     sandbox and assertions
  cases/                     one file per test group
```

Data that used to be embedded in the programs now sits next to the palette:

```
theme/
  palette.toml               colours, roles, terminal and app sections
  fonts.toml                 font roles and per-application opt-in
  maps/gtk.toml              GTK @define-color name -> role
  maps/kde.toml              KColorScheme groups, foregrounds, selection
  maps/eza.toml              file-type category -> extensions
  maps/catppuccin.toml       upstream Catppuccin hex -> palette name
  maps/obsidian.toml         Obsidian CSS custom property -> colour
```

`theme/` holds colour and font data only. Every config that carries colour is a
normal tracked dotfile in its own package; the generator stamps values into it
rather than rendering it from a template.

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

Fastfetch remains the primary detector. Targeted NVIDIA telemetry enriches
matching devices with live VRAM, utilization, clock and power readings from
`nvidia-smi`. Optional probe failures become health findings and never prevent
the remaining snapshot from rendering. Static components such as the cooler,
memory kit, case and power supply come from `hardware.dotfile` when firmware
interfaces cannot expose them without elevated privileges. The active profile
defaults to `desktop` on Linux, `macos` on Darwin and `windows` on Windows;
`SYSINFO_HARDWARE` can select an explicit profile.

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

- **A `targets` entry points inside the package.** The entry has to be able to
  land somewhere else, so the parent must be a real directory.
- **The destination is a directory that must never itself become a symlink** —
  `$HOME`, `~/.config`, `~/.local`, `~/.local/share`, `~/.local/bin`,
  `~/.config/systemd`, `~/.config/systemd/user`. Replacing any of these with a
  symlink into the repo would capture every unrelated file the system later
  writes there.

When a directory that was previously folded stops qualifying, it is *unfolded*:
the directory symlink is replaced by a real directory containing one symlink per
entry. This is why `link` is safe to re-run after adding a `targets` entry.

Groups are linked in manifest order, and a later group may hold a package of the
same name as an earlier one. The shared copy is linked first, then unfolded file
by file, and each file the later group also carries replaces its link. So a
platform group overrides individual files of a shared package while inheriting
the rest — how `shared/fastfetch` is specialised per platform.

### targets

`targets` maps a repo path to a destination. Without an entry, a package lands
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
- **tools, fonts, files, optional** — the entries in `requires.dotfile` for the
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

### requires.dotfile

Hand-maintained, one block per group, in the same grammar as
`packages.dotfile`:

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
a later commit removed. Both fail closed when `scripts/.venv` is missing.
Allowed false positives live in `scan.dotfile`; canary and invariant findings
cannot be allowed.

`init` writes this machine's identity, `enroll` and `revoke` edit
`keys.dotfile`, `sync` regenerates `.sops.yaml` from it (`--rewrap` also runs
`sops updatekeys` over every encrypted file), `keys` lists what is enrolled,
and `doctor` checks the whole chain from binaries through hooks.
`keys.dotfile` is the source of truth and `.sops.yaml` is generated and staged
by `pre-commit`, the same relationship `packages.dotfile` has with
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

`link` runs `apply` as its final phase. A machine with no age identity reports
its secrets as `sealed` and links everything else, so an unenrolled machine is
still usable.

See `docs/secrets.md` for the threat model behind all of it.

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

Generated colour files (`colors-*.conf`) are formatted as plain so the
generator's own column alignment survives.

---

## generate-theme

`theme/palette.toml` is the single source of truth for colour. This stamps it
into every config that carries colours.

```
generate-theme                        regenerate everything
generate-theme --check                report what would change, exit 1 if anything would
generate-theme --list-outputs         every file the generator owns
generate-theme --list-outputs --stageable   only the ones safe to auto-stage
```

### Colour indirection

Three layers. `[palette]` holds named colours (`mauve`, `base`). `[roles]` maps
a semantic name to a palette name (`prompt_git = "green"`). `[kde]` does the
same for the KDE/Qt scheme roles. Configs reference roles, so recolouring means
editing one role rather than hunting hex values.

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
theme/palette.toml         the colours
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

Colours used by the structural CSS outside that block (callout tints,
`::selection`, tab shadows) stay as `@name@` placeholders in the template,
because they are part of a rule rather than a custom property.

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
known upstream Catppuccin colour (Mocha or Macchiato) are rewritten. Anything
else is left alone, because widget placeholders and gradient defaults share the
same syntax and rewriting them would corrupt unrelated settings.

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
