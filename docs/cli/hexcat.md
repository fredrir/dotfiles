# hexcat

## Commands

<!-- cli:commands:start -->
| Command  | Description                                                                             |
| -------- | --------------------------------------------------------------------------------------- |
| `hexcat` | Prints files like cat; on a terminal with built-in bat highlighting and color swatches. |
<!-- cli:commands:end -->

## Flags

<!-- cli:flags:start -->
| Flag                       | Description                                                          |
| -------------------------- | -------------------------------------------------------------------- |
| `-A`, `--show-all`         | Shows nonprinting characters, tabs and line ends.                    |
| `-b`, `--number-nonblank`  | Numbers nonempty lines.                                              |
| `-e`                       | Same as `-vE`.                                                       |
| `-E`, `--show-ends`        | Shows `$` at line ends.                                              |
| `-l`                       | Locks stdout while writing.                                          |
| `-n`, `--number`           | Numbers all lines.                                                   |
| `-s`, `--squeeze-blank`    | Squeezes repeated empty lines into one.                              |
| `-t`                       | Same as `-vT`.                                                       |
| `-T`, `--show-tabs`        | Shows tabs as `^I`.                                                  |
| `-u`                       | Writes output unbuffered.                                            |
| `-v`, `--show-nonprinting` | Shows nonprinting characters.                                        |
| `--color <WHEN>`           | Chooses `auto`, `always`, or `never` colors; `never` runs plain cat. |
| `--filter`                 | Adds swatches to the input without running cat or bat.               |
| `--pager`                  | Adds swatches, then pages through less; set as `BAT_PAGER`.          |
| `-h`, `--help`             | Shows help for the selected command and exits.                       |
| `--completions <SHELL>`    | Prints a shell completion script for the named shell and exits.      |
| `-V`, `--version`          | Prints the version and exits.                                        |
<!-- cli:flags:end -->

## Routes

| Output                               | Runs                        |
| ------------------------------------ | --------------------------- |
| Piped, or `--color never`            | system cat                  |
| Terminal                             | built-in bat, then swatches |
| Terminal with `-b -e -E -l -t -T -v` | system cat, then swatches   |
| `--filter`, `--pager`                | swatches only               |

## Env

| Env               | Default                                  |
| ----------------- | ---------------------------------------- |
| `BAT_THEME`       | `auto`; asks the terminal for light/dark |
| `BAT_THEME_DARK`  | `Monokai Extended`                       |
| `BAT_THEME_LIGHT` | `Monokai Extended Light`                 |
| `COLORTERM`       | unset; `truecolor` or `24bit` for 24-bit |
