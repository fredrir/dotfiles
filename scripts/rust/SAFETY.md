# Rust safety

| Policy | Enforcement |
|---|---|
| Workspace default | `unsafe_code = "deny"` |
| Safe binary, library, and test targets | `#![forbid(unsafe_code)]` |
| Unsafe operations inside unsafe functions | `unsafe_op_in_unsafe_fn = "deny"` |
| Remaining unsafe blocks | `clippy::undocumented_unsafe_blocks = "deny"` |
| Environment overrides | Context values and `Command::env` / `envs` |
| Dependency internals | Outside the first-party unsafe count |

| Boundary | Required invariants |
|---|---|
| `size/src/bulk.rs` | Owned directory descriptor; exclusive writable batch buffer; borrowed names consumed before reuse |
| `size/src/bulk_decode.rs` | Safe slices; checked record lengths and offsets; names bounded to their record |
| `workstation/src/screen.rs` | Static atomic cancellation flags; signal-safe handler; restore dispositions and terminal before reraising |
| `hostkit/src/process.rs` | Child-only, signal-safe `setsid` before exec |
| `testkit/src/pty.rs` | Child-only session and controlling-terminal setup before exec |
| `sysinfo-collect/src/macos.rs` | Native handle ownership and typed FFI contracts; matching release for owned references |
| `agent-hop/src/handoff/mod.rs` | Lease stays locked until every inherited descriptor closes |
| `flatten/src/dir.rs`, `agent-hop/src/handoff/snapshot.rs` | Descriptor-relative operations; retain no-follow and directory checks |

Run from `scripts/rust`:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace --lib --bins --tests
SIZE_FUZZ_CASES=1000000 SIZE_FUZZ_SEED=12345 cargo test -p size --test bulk_decode fuzz_decoder -- --ignored --exact
```

| Validation | Scope |
|---|---|
| macOS and Linux | Filesystem, process, terminal, environment, and decoder tests |
| Decoder fuzzing | Seeded mutations of valid records, random bytes, and truncations; portable to Linux |
| `size` performance | Interleave baseline and changed release binaries against the same tree; compare output and median time |
| Standalone benchmarks | Run separately from nextest; their custom harnesses do not support test enumeration |
