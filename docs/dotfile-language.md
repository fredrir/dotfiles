# The `.dotfile` Language

The block format `dotfmt` lays out, and what the same tool does to `.conf` and
`.config`. See [cli/dotfmt.md](./cli/dotfmt.md) for the command itself.

## Lines

| Line          | Class   |
| ------------- | ------- |
| (empty)       | Blank   |
| `# anything`  | Comment |
| `name {`      | Open    |
| `}`           | Close   |
| `key = value` | Entry   |
| anything else | Bare    |

| Rule            | Value                                                        |
| --------------- | ------------------------------------------------------------ |
| Nesting         | One level; a block inside a block is refused                 |
| Entry           | Splits on the first `=`, both sides trimmed                  |
| Trailing `#`    | Part of the value, never a comment                           |
| Top level       | An entry outside a block is legal                            |
| Comments        | Kept, never stripped                                         |
| Line ends       | `\r\n` is read, `\n` is written                              |

| Body                  | Error                  |
| --------------------- | ---------------------- |
| `}` with nothing open | `unexpected }`         |
| A block inside one    | `nested block`         |
| A block left open     | `missing } for <name>` |

Failures are reported as `<file>:<line>: <message>`.

## Layout

| Setting         | Default |
| --------------- | ------- |
| `indent`        | `2`     |
| `align`         | `true`  |
| `align_max`     | `24`    |
| `blank_lines`   | `1`     |
| `final_newline` | `false` |

`final_newline` governs `.dotfile`, `.conf`, and `.config` alike: at the built-in
and shipped default of `false`, the last line carries no newline. The other four
settings lay out `.dotfile` only.

- A group is a run of entries, bare lines, and comments inside a block. A blank
  line starts a new group; a comment does not.
- The `=` sits two columns past the widest key in the group. A key at or past
  `align_max` takes its one space and overflows rather than widening the column.
- Only entries set the width. A bare line neither pads nor widens, and a group
  holding only bare lines is left as it stands.
- Top-level entries are normalized to `key = value` and never aligned, because
  `dotfile add` tests `config/targets.dotfile` for that exact string.
- An entry with no value gets no trailing space, and a block header is always
  re-emitted as `name {`.
- Blank lines go at the edges and above `}`, and collapse to at most
  `blank_lines` in between.
- Interior whitespace inside a value is never touched. A file of nothing but
  blank lines is left exactly as it was.

```
host {
# a label
a = 1
bb = 2

longer_key = 3
c = 4
}
```

```
host {
  # a label
  a   = 1
  bb  = 2

  longer_key  = 3
  c           = 4
}
```

## `dotfmt.dotfile`

| Order | Path                                                     |
| ----- | -------------------------------------------------------- |
| 1     | The nearest `dotfmt.dotfile` at or above the target      |
| 2     | `$XDG_CONFIG_HOME/dotfmt/dotfmt.dotfile`                 |
| 3     | `$HOME/dotfmt.dotfile`                                   |
| 4     | The built-in defaults                                    |

Patterns anchor to the directory holding the config. The two home locations are
not above anything in particular, so their patterns are read from `/`.

| Block     | Holds                                          |
| --------- | ---------------------------------------------- |
| `dotfmt`  | The layout settings above, as `key = value`    |
| `include` | Which files dotfmt owns, one pattern per line  |
| `exclude` | Applied after `include`, one pattern per line  |

A setting outside a block, an unknown block, and an unknown key are all refused.
So is a pattern holding an `=`, which the grammar would read as an entry and
this file would then rewrite.

`shared/tools/dotfmt.dotfile` is this repository's config and a worked example.

## Selection

| Token      | Picks up                                              |
| ---------- | ----------------------------------------------------- |
| `.conf`    | `*.conf`                                              |
| `.config`  | `*.config`                                            |
| `.dotfile` | `*.dotfile`                                           |
| `_empty_`  | A name with no extension, `.conf` and `LICENSE` alike |

An include entry is `[!][<directory pattern>/]<token>`. It must end in a token:
`*.conf`, `kitty`, and `**ssh/` are all refused.

| Entry             | Selects                                              |
| ----------------- | ---------------------------------------------------- |
| `.conf`           | Everywhere                                           |
| `/.conf`          | Only beside the config                               |
| `/deep/.conf`     | Only in `deep/`                                      |
| `one/**/.conf`    | `one/` and everything below it                       |
| `ssh/_empty_`     | Under a directory named exactly `ssh`, at any depth  |
| `**ssh/_empty_`   | `*ssh`, so `.ssh` and `openssh` come too             |
| `!**/kitty/.conf` | Takes back what an earlier entry gave                |

`.dotfile` everywhere is included before any config is read; `!.dotfile` takes
it away. The last matching entry wins.

Exclude entries are plain `.gitignore` patterns, applied after include, and the
last matching one wins. An excluded directory takes everything below it, so a
`!` cannot lift one file back out of it. An exclude entry holding a token is
refused, since `exclude { .conf }` means a file *named* `.conf`. No pattern may
hold an `=` or end in `\`, which would escape a trailing space the parser has
already trimmed away.

## `.conf` and `.config`

| Pattern                                                                 | Mode    |
| ----------------------------------------------------------------------- | ------- |
| `*/hypr/*`, `*/hypr-local.conf`, `hypr*.conf`                           | `hypr`  |
| `*/kitty/colors*.conf`, `*/colors*.conf`, `*/kitty/conf.d/fonts.conf`   | `plain` |
| `*/kitty/*.conf`, `*/kitty.conf`                                        | `kitty` |
| anything else                                                           | `plain` |

The table is compiled in and no config can remap it. It is read `hypr`, then
`plain`, then `kitty`, so a plain pattern beats the kitty pattern it sits
inside.

| Mode    | Does                                                                   |
| ------- | ---------------------------------------------------------------------- |
| `plain` | Trims line ends, drops blank lines at the edges, collapses runs to one |
| `hypr`  | Plain, plus four spaces per open block and `key = value` normalization |
| `kitty` | Compacts whitespace outside quotes and lays out two columns            |

`kitty` measures `map` shortcuts against each other and every other key against
the rest, so the two columns move independently. A comment is never compacted,
and a `map` line with no action is left as it was found.

In `hypr`, a `{` inside a comment or a value does not open a block, and the
blank line above a `}` goes the way it does in a `.dotfile`. A file of only
whitespace is left exactly as it was, and `final_newline` ends the file the way
it ends a `.dotfile`.

## Writing

| Check     | Value                                                            |
| --------- | ---------------------------------------------------------------- |
| Settles   | Laying the result out again must produce the result              |
| Signature | Every non-blank line's class, block, key, and value must survive |

Either check failing writes nothing and reports an internal error. A write goes
to a sibling temporary file and is renamed over the original, keeping its mode,
and a file reached through a symlink is written through it.
