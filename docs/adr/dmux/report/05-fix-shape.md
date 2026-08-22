# Recommended fix shape

Part of the [dmux epoch-verification integrity review](INDEX.md). Reviewed tree `493e92c`. Answers review-ask #3 (decide the correct fix shape).

---

**Choose: promote `ls_cli`'s `ScanTarget`/`ManagedScope` resolver into `backend/` as the sole way to
obtain a scope for a registry instance, and make `InventoryScope.expected_epoch` private behind two
constructors — `managed(backend, endpoint, epoch: ServerEpoch)` and `unmanaged_endpoint(backend,
endpoint)`.** The boundary goes at the **resolver**, not at the constructor, not at the provider
entry point, and not at the type of the field.

## Why that boundary and not the other three

*Not the enum (`EpochExpectation::{Pinned, Unmanaged}`).* Measured cost: 70 mechanical edits across
29 files. Measured benefit: **zero sites closed automatically**. Every laundering site has a total,
compiling, one-line migration that preserves the bug —
`.map_or(EpochExpectation::Unmanaged, EpochExpectation::Pinned)`. It buys a greppable annotation, and
in production `src` there is exactly **one** honest `Unmanaged` caller (`ls_cli.rs:846`) against
nine dishonest ones, so the second variant is a default-looking hatch that will regrow the class.
Worth doing as a *commit-1 refactor* under the recommended shape; not worth doing as the shape.

*Not the constructor alone.* Private fields make `expected_epoch: <whatever>` unwritable, which is
necessary — but the `Option` originates upstream in `backend_server(..).server_epoch`, and that row
is also read for pid/start_token/dev/ino at `gui_cli.rs:1348` and `gui_lifecycle.rs:963`, so it
cannot be withdrawn. A launderer still satisfies the compiler with
`match epoch { Some(e) => ::managed(..), None => ::unmanaged_endpoint(..) }`. Constructors are where
you *notice*; they are not where you *decide*.

*Not the provider entry point (`inventory_verified` / `inventory_discover`).* This is the only shape
that closes sites **automatically** — 9 launders become 9 compile errors, plus it closes the six
adapter-internal wez holes (`group_rename`, `group_remove`, `split_new`, `split_remove`,
`split_list`, `normalize_plan`) that no enumerated list contains. That is a real advantage and I
weighed it seriously. It loses on two grounds. First, blast radius: 21 trait methods, 4 impls, 76
`.inventory(` call sites, and — decisively — the **63 `scope(None)` call sites across ~55 adapter
unit tests** that exist precisely to exercise the unpinned path and would all need re-expressing
against `discover`. There is no incremental version; it is a flag day. Second, it does not solve the
actual problem, because two of the nine sites (`main.rs:1453`, and any future site that never
consults the registry) have no registry lookup to constrain — the split would force them to write
`DiscoveryScope::tmux_namespace(namespace)`, which compiles and is wrong. Take it as **phase 2**,
after the resolver has burned the list down, if residual risk still bites.

*Take the resolver.* It is the only boundary where the *decision* lives — the point at which
"a managed instance's epoch is NULL" is first observable — and it is the shape 493e92c already
proved out in one file. Promoting `ManagedTarget { Managed{instance, scope}, Unpublished(uid),
Unaddressable(uid), Unregistered }` plus
`resolve_managed(&Registry, Backend) -> Result<ManagedTarget, _>` into `backend/scope.rs` means the
enum contains **no `Option<ServerEpoch>` anywhere**, so in the `Unpublished` arm there is no
`ServerEpoch` value in scope to hand to `managed()`. All 9 launder sites go from "wrote nothing" to
"must type a distinct branch, in a function that resolved an instance three lines earlier". It also
deletes seven near-duplicate resolvers (`migrate_cli::scan_target`, `adopt_cli::owner_scope`,
`rm_cli::local_scope`, `new_cli::local_target`, `gui_cli::local_opposite_create_target`,
`agent::owner_lookup_target`, `space_cli::reconcile_provider`'s tmux arm), which is where the
divergence came from in the first place.

One prerequisite that is load-bearing and easy to miss: **move `InventoryScope` out of
`backend/mod.rs` into `backend/scope.rs` first.** `backend::wez` and `backend::tmux` are child
modules, so a private field in `mod.rs` is still fully visible to them — verified experimentally:
with the field made private, `cargo check` reported E0451 at 27 lib sites but *not* at
`wez.rs:2937` or `tmux.rs:2136`.

## Blast radius (measured, not estimated)

Compiler-verified by `rsync` to scratch + `cargo check -p dmux --all-targets` (baseline green):
**27 struct-literal sites (E0451) + 7 field-read sites (E0616) in the lib target**, plus 7 literals
and 1 read in the bin target (grep-derived; never compiled because the lib failed first), plus 18
literals and 1 read in `tests/`. **61 edit points total**, 2 exempt by module-descendant privacy.
Test churn is small — 31 of the 54 field-inits are test code, all one-token substitutions with no
assertion changes, provided the three pass-through `fn scope(expected: Option<..>)` helpers
(`wez.rs:2936`, `tmux.rs:2135`, `tests/provider_tmux.rs:122`) keep their signatures and convert in
the body. Two assertions do break: `wez.rs:3800` and `tmux.rs:2498` assert
`detail.contains("expected_epoch")` against the refusal strings, so reword those messages
deliberately or leave them alone. Baseline `cargo check -p dmux --all-targets` is 12.6s incremental.

## Migration order — no flag day for steps 1-4

1. Move `InventoryScope` to `backend/scope.rs`, `pub use` it. Zero behaviour.
2. Private `expected_epoch` + `managed()` / `unmanaged_endpoint()` + accessor. One commit, 61 edit
   points, suite stays green. From here `grep -rn 'unmanaged_endpoint' src/` is a standing audit
   list of ~11 entries (2 legitimate, 9 suspect); today the equivalent grep is `expected_epoch` at
   111 hits with no signal. Add a CI grep gate.
3. Land `resolve_managed` and `ManagedTarget`.
4. Migrate the 9 launder sites **one commit each**, each answering "what does `Unpublished` mean for
   this verb" explicitly: refuse for adopt/migrate/new/rm; `Unreachable` row for ls/spaces; refuse
   for the gui opposite-create hint; refuse for reconcile. Copy the error text that already exists
   at `space_cli.rs:1033` and the code mapping at `ls_cli.rs:1209` (`BackendEpochChanged`).
5. Fix `main.rs:1450` by *adding* the two missing comparisons — endpoint vs
   `backend_instance_info(..).socket_path`, and live epoch vs `backend_server(..).server_epoch` —
   copying `operations::validate_marker_context` (`operations.rs:3161-3181`), 50 lines above the
   function that lacks them.
6. Separately and independently: add `required_action_epoch` to the nine unfenced wez verbs. The
   resolver does not fix those; nothing at the scope boundary does.

## Residual risk after the fix

The resolver cannot reach a site that never consults the registry, so `main.rs:1450` stays a
hand-fix (step 5) and a future ambient-endpoint site would reintroduce the class. It proves the scan
matches the **registry**, not that the registry matches reality — finding #1 survives untouched and
needs its own liveness check. It leaves `native_bindings.server_epoch` write-only, so the ten
`binding_epoch` cells remain tautologies until `BINDING_COLUMNS` is extended. And it does nothing
about `tests/provider_contract.rs:82`, which will keep teaching new adapters that `None` means skip.
