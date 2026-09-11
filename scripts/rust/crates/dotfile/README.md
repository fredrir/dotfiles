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
| Remote sync | Matching native push protocol; install with `./setup.sh --commands-only` |
| Interrupted mutations | Recovered under the next mutation lock; ambiguous or edited paths are preserved |
| Python tools | Remain separate; setup exports `config/command-surface.json` |
| Transcript redaction | Native JSON-lines helper; plaintext values stay inside dotfile |
| Sysinfo colors | Native versioned palette JSON |
| Commit/push review | `i` inspect, `a` accept exact contents, `q` / Enter abort |
| Accepted findings | Local `.git/dotfile/scan-approvals.json`; path, SHA-256, pattern labels only |
| Review limits | Canary/encryption violations block; CI and unavailable terminals fail closed |

```sh
cargo build --release --locked --manifest-path scripts/rust/Cargo.toml -p dotfile-cli
dotfile dev check -l rust -p dotfile-cli
dotfile dev test -l python -p dotfile,surface,transcript,utils
```

| Contract | Coverage |
| --- | --- |
| All theme profiles and emitters | `tests/unit/theme_tests.rs`, `tests/theme_cli.rs` |
| Color expressions | `tests/unit/theme_tests.rs` |
| Picker, preview, signals, tmux | `tests/theme_cli.rs` |
| Real encryption, rotation, revocation, recovery, Git history | `tests/secret_e2e.rs` |
| Interactive review, exact staged contents, approvals, commit/push hooks | `tests/secret_scan_review.rs` |
| System ownership/modes, add/remove, doctor | `tests/system_manage_doctor.rs` |
| Native metadata, missing/stale exports, permissions, locks | `tests/surface_native.rs` |
| JSONC spans, merge decisions, ignore patterns | `tests/unit/sync/merge_tests.rs`, `tests/native_sync_core.rs` |
| Comment-preserving JSONC edits and adoption | `tests/unit/sync/adoption_tests.rs` |
| Sync UI and remote protocol | `tests/native_sync_core.rs`, `tests/push_path.rs`, `tests/sync_*` |
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

```sh
# Review the Git index; approvals apply to identical blobs in later pushes.
dotfile secret scan --staged --review
# Review outgoing ranges together.
dotfile secret scan --commits 'origin/main..HEAD' --review
# Forget local approvals.
rm -f "$(git rev-parse --git-path dotfile/scan-approvals.json)"
```
