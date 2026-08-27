# dotfile theme

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