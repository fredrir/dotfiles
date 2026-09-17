# dotfile

| Area                                                    | Source                                 |
| ------------------------------------------------------- | -------------------------------------- |
| Command tree, dispatch                                  | `src/cli.rs`                           |
| Profiles, targets, manifests, block syntax              | `src/config/`                          |
| Atomic writes, recovery journals, path resolution       | `src/fs.rs`, `src/fs/transaction.rs`   |
| Repository and workstation locks                        | `src/lock.rs`                          |
| Cancellable processes, terminal ownership               | `src/process.rs`, `hostkit::process`   |
| SOPS, age, recipients, templates, scanning              | `src/secret/`                          |
| Root-owned file inspection and installation             | `src/system/`                          |
| Add/remove, adoption, Git staging                       | `src/manage/`                          |
| Palette model, validation, emitters, picker             | `src/theme/`                           |
| Workstation checks                                      | `src/doctor/`                          |
| Reconciliation and remote sync                          | `src/sync/`, `src/push/`               |
| Profile and override selection                          | `src/sync/selection.rs`                |
| Toolchain discovery, build, install                     | `src/tooling/`                         |
| Docs, keybind parsers, README capture, benchmark tables | `src/docs/`                            |
| Package inventory                                       | `src/artifacts/packages.rs`            |
| Metadata and completion providers                       | `src/surface/`, `workstation::surface` |
| Authored reference text                                 | `assets/cli-reference.json`            |

| Runtime                | Requirement                                                                                   |
| ---------------------- | --------------------------------------------------------------------------------------------- |
| Dotfile commands       | Native Rust executable                                                                        |
| Encryption             | External `sops` and `age-keygen`                                                              |
| Git operations         | External `git`; literal pathspecs and bounded blob batches                                    |
| System installation    | Linux, `sudo install`; inspection and dry-run also work on macOS                              |
| Completion shell       | Zsh                                                                                           |
| Remote sync            | Matching native push protocol; install with `./setup.sh --commands-only`                      |
| Bootstrap              | `setup.sh` builds `dotfile`; `dotfile sync` installs everything else                          |
| Interrupted mutations  | Recovered under the next mutation lock; ambiguous or edited paths are preserved               |
| Python tools           | Standalone Hyprland/transcript; declarative command metadata in `config/command-surface.json` |
| Rust runtime           | No Python imports, launchers, or exporters                                                    |
| Documentation writes   | Render/validate all outputs, then durable transaction; unchanged files retain timestamps      |
| Documentation defaults | CLI, keybinds, packages; README and benchmarks require `--only`                               |
| Transcript redaction   | Native JSON-lines helper; plaintext values stay inside dotfile                                |
| Sysinfo colors         | Native versioned palette JSON                                                                 |
| Commit/push review     | `i` inspect, `a` accept exact contents, `q` / Enter abort                                     |
| Accepted findings      | Local `.git/dotfile/scan-approvals.json`; path, SHA-256, pattern labels only                  |
| Review limits          | Canary/encryption violations block; CI and unavailable terminals fail closed                  |

```sh
cargo build --release --locked --manifest-path scripts/rust/Cargo.toml -p dotfile-cli
dotfile dev check -l rust -p dotfile-cli
dotfile dev test -l python -p dotfile,hyprland,transcript
```

| Contract                                                                | Coverage                                                          |
| ----------------------------------------------------------------------- | ----------------------------------------------------------------- |
| All theme profiles and emitters                                         | `tests/unit/theme_tests.rs`, `tests/theme_cli.rs`                 |
| Color expressions                                                       | `tests/unit/theme_tests.rs`                                       |
| Picker, preview, signals                                                | `tests/theme_cli.rs`                                              |
| Real encryption, rotation, revocation, recovery, Git history            | `tests/secret_e2e.rs`                                             |
| Interactive review, exact staged contents, approvals, commit/push hooks | `tests/secret_scan_review.rs`                                     |
| System ownership/modes, add/remove, doctor                              | `tests/system_manage_doctor.rs`                                   |
| Native/declarative metadata, permissions, locks                         | `tests/surface_native.rs`                                         |
| Documentation checks, JSON/diff, atomicity, no-Python operation         | `tests/docs_cli.rs`                                               |
| Keybind parsers, platform deduplication, source links                   | `tests/docs_keybinds.rs`                                          |
| JSONC spans, merge decisions, ignore patterns                           | `tests/unit/sync/merge_tests.rs`, `tests/native_sync_core.rs`     |
| Comment-preserving JSONC edits and adoption                             | `tests/unit/sync/adoption_tests.rs`                               |
| Sync UI and remote protocol                                             | `tests/native_sync_core.rs`, `tests/push_path.rs`, `tests/sync_*` |
| Python adapters, hooks, bootstrap                                       | `scripts/python/tests/`                                           |

```sh
dotfile docs
dotfile docs --check
dotfile docs --only keybinds --diff
dotfile docs --only cli,keybinds --check --json
dotfile docs --only readme
dotfile docs --only benchmarks
scripts/rust/target/debug/dotfile --completions zsh | zsh -n

# Install the workstation commands.
./setup.sh --commands-only
```

```sh
# Review the Git index; approvals apply to identical blobs in later pushes.
dotfile secret scan --staged --review
# Review outgoing ranges together.
dotfile secret scan --commits 'origin/main..HEAD' --review
# Forget local approvals.
rm -f "$(git rev-parse --git-path dotfile/scan-approvals.json)"
```

## Toolchain

`setup.sh` compiles `dotfile` and execs `dotfile sync`, the single entry point
for installs, updates and reconciliation. Every other command is built and
installed by `src/tooling/` as sync's first phase.

| Language | Built from       | Binaries derived from                           | Driver  |
| -------- | ---------------- | ----------------------------------------------- | ------- |
| Rust     | `scripts/rust`   | workspace `[[bin]]` targets, else `src/main.rs` | `cargo` |
| Python   | `scripts/python` | `[project.scripts]`                             | `uv`    |
| Go       | `scripts/go`     | `cmd/<name>`                                    | `go`    |

| Behaviour               | Value                                                      |
| ----------------------- | ---------------------------------------------------------- |
| Staleness               | sha256 of inputs, stamped in `config/sync/<language>`      |
| Build order             | all stale languages in parallel                            |
| Install                 | one `fs::transaction`; binaries and stamps commit together |
| Unchanged binary        | left in place, keeps its mtime                             |
| Missing `cargo` or `uv` | fatal                                                      |
| Missing `go`            | warns, skips the Go commands                               |

| Invocation                     | Effect                                                  |
| ------------------------------ | ------------------------------------------------------- |
| `./setup.sh`                   | build `dotfile`, then `dotfile sync`                    |
| `dotfile sync`                 | install stale commands, then reconcile                  |
| `dotfile sync --commands-only` | install commands, stop                                  |
| `dotfile sync --native-only`   | compiled commands only, no Python, stop                 |
| `dotfile sync --rebuild`       | rebuild regardless of stamps                            |
| `dotfile sync --macos`         | profile spelled as a flag; same as `dotfile sync macos` |

Prompts for a profile and its machine overrides only when none is saved and the
terminal is interactive; a saved profile or an explicit one never prompts.
