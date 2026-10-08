# zsh-build

## Commands

<!-- cli:commands:start -->
| Command     | Description                                                                          |
| ----------- | ------------------------------------------------------------------------------------ |
| `zsh-build` | Compiles the zsh startup config into one bundle with fixed command output folded in. |
<!-- cli:commands:end -->

## Flags

<!-- cli:flags:start -->
| Flag                    | Description                                                             |
| ----------------------- | ----------------------------------------------------------------------- |
| `-n`, `--dry-run`       | Shows whether the bundle would change without writing it.               |
| `--root <ROOT>`         | Sets the dotfiles checkout; defaults to `$DOTFILES`, then `~/dotfiles`. |
| `-h`, `--help`          | Shows help for the selected command and exits.                          |
| `--completions <SHELL>` | Prints a shell completion script for the named shell and exits.         |
| `-V`, `--version`       | Prints the version and exits.                                           |
<!-- cli:flags:end -->

## Files

| Path | Value |
| --- | --- |
| `config/zsh/build.toml` | Targets, ambient variables, commands folded at build time |
| `.cache/zsh/build/zshrc.zsh` | Bundle; built by `dotfile sync` |
| `.cache/zsh/build/zshrc.zsh.zwc` | Wordcode; compiled in the background by the first shell after a build |

## Switches

| Name                | Value                                          |
| --- | --- |
| `ZSH_BUILD_SKIP=1`  | Loads the `shared/zsh` sources instead         |
| `# zsh-build: omit` | Drops the statement that follows from a bundle |