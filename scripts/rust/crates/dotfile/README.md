# dotfile

| Area | Source |
| --- | --- |
| Command tree, dispatch | `src/cli.rs` |
| Profiles, targets, manifests, block syntax | `src/config/` |
| Atomic writes, recovery journals, path resolution | `src/fs.rs`, `src/fs/transaction.rs` |
| Repository and workstation locks | `src/lock.rs` |
| Cancellable processes, terminal ownership | `src/process.rs`, `hostkit::process` |
| SOPS, age, recipients, templates, scanning | `src/secret/` |
| Root-owned file inspection and installation | `src/system/` |
| Add/remove, adoption, Git staging | `src/manage/` |
| Palette model, validation, emitters, picker | `src/theme/` |
| Workstation checks | `src/doctor/` |
| Reconciliation and remote sync | `src/sync/`, `src/push/` |
| Docs, packages, completions | `src/artifacts/`, `src/surface/` |
| Authored reference text | `assets/cli-reference.json` |

| Runtime | Requirement |
| --- | --- |
| Dotfile commands | Native Rust executable |
| Encryption | External `sops` and `age-keygen` |
| Git operations | External `git`; literal pathspecs and bounded blob batches |
| System installation | Linux, `sudo install`; inspection and dry-run also work on macOS |
| Completion shell | Zsh |
| Interrupted mutations | Recovered under the next mutation lock; ambiguous or edited paths are preserved |
| Python tools | Remain separate; setup exports `config/command-surface.json` |
| Transcript redaction | Native JSON-lines helper; plaintext values stay inside dotfile |
| Sysinfo colors | Native versioned palette JSON |

```sh
cargo build --release --locked --manifest-path scripts/rust/Cargo.toml -p dotfile-cli
cargo test --locked --manifest-path scripts/rust/Cargo.toml -p dotfile-cli
cargo test --locked --manifest-path scripts/rust/Cargo.toml -p hostkit -- --test-threads=1
cargo clippy --locked --manifest-path scripts/rust/Cargo.toml -p dotfile-cli -p hostkit --all-targets -- -D warnings
uv run --project scripts/python --locked pytest scripts/python/tests/dotfile scripts/python/tests/surface scripts/python/tests/transcript scripts/python/tests/utils/sysinfo
```

| Contract | Coverage |
| --- | --- |
| All theme profiles and emitters | `tests/fixtures/theme/oracle.json`: 160 byte-exact outputs |
| Color expressions | `tests/fixtures/theme/expressions.json` |
| Picker, preview, signals, tmux | `tests/theme_cli.rs` |
| Real encryption, rotation, revocation, recovery, Git history | `tests/secret_e2e.rs` |
| System ownership/modes, add/remove, doctor | `tests/system_manage_doctor.rs` |
| Native metadata, missing/stale exports, permissions, locks | `tests/surface_native.rs` |
| JSONC spans, merge decisions, ignore patterns | `tests/fixtures/merge-legacy.json` |
| Comment-preserving JSONC edits and adoption | `tests/unit/sync/adoption_legacy_tests.rs` |
| Existing sync UI and remote behavior | `tests/native_sync_core.rs`, `tests/push_path.rs`, `tests/sync_*` |
| Python adapters, hooks, bootstrap | `scripts/python/tests/` |

```sh
# Refresh Python metadata after changing Python commands.
uv run --project scripts/python --locked python -m tools.surface.export
scripts/rust/target/debug/dotfile __reference
scripts/rust/target/debug/dotfile __reference --check
scripts/rust/target/debug/dotfile --completions zsh | zsh -n

# Install the workstation commands.
./setup.sh --commands-only
```

## Measurements

macOS arm64, release builds, warm cache; startup and child processes included.

| Operation | Python median | Rust median | Ratio |
| --- | ---: | ---: | ---: |
| Theme dry-run, same checkout | 2358 ms | 22.1 ms | 106.6× |
| Secret scan, 1,000 worktree files | 332 ms | 47.5 ms | 7.0× |
| Secret scan, 1,000 staged files | 9917 ms | 187 ms | 52.9× |

| Staged scanner | Python | Rust |
| --- | ---: | ---: |
| Git processes | 1,002 | 5 |
| Peak RSS | 33.6 MB | 17.1 MB |

Raw samples and reproduction: `tests/fixtures/theme/benchmark.json`, `tests/fixtures/secret-scan-benchmark.json`, `tests/fixtures/startup-benchmark.json`.

```sh
cargo test --release --locked --manifest-path scripts/rust/Cargo.toml -p dotfile-cli --test theme_cli benchmark_theme_commands -- --ignored --nocapture
```
