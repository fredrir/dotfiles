## Rules
- Let me know first before implementation if anything is unclear or if you have any questions

## Conventions
- **Shared UI / TUI Library:** [crates/ui](./scripts/rust/crates/ui/README.md)
- **Rust Safety:** [SAFETY.md](./scripts/rust/SAFETY.md)

## Testing and Linting
- Use `dotfile dev [test | lint | check] [COMMANDS]`
- Prefer running tests scoped and relevant to the changes
- Avoid running the full test-suite unless needed

### Options
- test   Run tests
- lint   Run linters
- check  Run linters and tests

### Commands
  `-p, --pkg <TARGET>`             Select packages; repeat or comma-separate
  `-l, --lang <LANGUAGE>`       Select languages; repeat or comma-separate [E.g python]
  `-n, --dry-run`                       Dry run
  `-v, --verbose`                       Verbose output
      `--changed[=<REF>`        Select affected packages and dependents; compare with HEAD or REF
      `--python-workers <N>` Max Python workers [default: 4]
  `-j, --jobs <N>`                      Total worker budget; defaults to CPU count
      `--concurrency <N>`        Maximum simultaneous tasks [default: 2]