# scripts

Reference for the workstation command-line tools in `scripts/`. Code files
carry no comments, so the reasoning behind the non-obvious behaviour lives
here.

`scripts/` is a uv-managed Python project (Typer + Rich, package name
`tools`). `uv sync --project scripts --locked` installs it into
`scripts/.venv`, whose `bin` directory the workstation zsh profiles put on
PATH. Each command is a console entry point declared in
`scripts/pyproject.toml`; there is no umbrella command. The project is
workstation-only: the Ubuntu server profile never syncs, imports, or invokes
it, and `bootstrap-vps.sh` links the server profile with its own standalone
shell linker.

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
      oc.py                  openclaw over an SSH tunnel
      sysinfo.py             environment and hardware summary via fastfetch
    desktop/
      power_menu.py          wofi power menu (Hyprland)
      confirm_exit.py        wofi exit confirmation (Hyprland)
      clean_paste.py         clipboard normaliser behind Ctrl+Shift+V
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

### fastfetch logo gradient

`arch.txt` is recoloured with a linear gradient interpolated across the four
section accent colours, one step per line of ASCII art. Existing escape codes
are stripped before recolouring so the operation is idempotent.

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

## clean-paste

Rewrites the clipboard as clean plain text: CRLF to LF, ANSI/OSC escapes
stripped, non-breaking and zero-width spaces normalised, stray control
characters dropped (tabs kept), trailing whitespace removed, leading and
trailing blank lines trimmed, and the longest whitespace prefix common to
every non-blank line removed, so relative nesting survives while the
terminal-UI indentation that tools like codex leave behind does not. Because
the cleaned text is re-copied, only `text/plain` is offered afterwards, which
is what strips rich-text formatting.

The desktop-wide binding lives in `linux/common/xremap/config.yml`, not in any
one application: xremap intercepts Ctrl+Shift+V, launches `clean-paste`,
sleeps 200ms (the command runs in ~50ms; the margin covers large clipboards),
then re-emits Ctrl+Shift+V. Re-emitting the same combination is what keeps the
binding application-neutral — terminals still see their native paste, GUI apps
still see paste-without-formatting — and it cannot loop because xremap never
watches its own virtual output device (the existing `Shift-Equal` rule relies
on the same property). If the venv is missing the launch fails silently and
the key degrades to a plain paste.

A non-text clipboard (an image), invalid UTF-8, or whitespace-only content is
left untouched.

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
