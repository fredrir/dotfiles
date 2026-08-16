//! Remote owner-agent surface (plan §12). `protocol` carries the frozen
//! versioned message contract (§12.1). `agent` and `attach` are the two
//! binary entry points wired into the hidden `_agent`/`_attach` subcommands
//! (ADR 009 §1). `client` is the transport + retry-matrix seam, `routes`
//! the route policy layer, `lineage` the §12.1 peer checkpoint policy,
//! `enroll` the `dmux ssh` flow, and `hosts` the `dmux host ls|label|
//! forget` library entry points the root wires into the CLI.

pub mod agent;
pub mod attach;
pub mod client;
pub mod enroll;
pub mod hosts;
pub mod lineage;
pub mod protocol;
pub mod routes;
pub mod wez_compat;

/// Non-interactive ssh sessions arrive with a POSIX locale on Linux, and a
/// POSIX-locale tmux CLIENT sanitizes the provider's U+001F field
/// separators to `_`, breaking every identity/inventory parse. The hidden
/// remote endpoints (`_agent`, `_attach`) therefore normalize their OWN
/// process locale to a UTF-8 codeset before spawning any tmux command.
/// A configured UTF-8 locale is always left untouched.
///
/// Called once at endpoint entry, before any threads exist (the
/// set-env safety requirement). Public because `_tmux-bootstrap` in the
/// binary shares the exposure when invoked over bare ssh (root W5
/// integration; see the P7 handoff risks).
pub fn normalize_utf8_locale() {
    let utf8 = |key: &str| {
        std::env::var(key)
            .map(|v| {
                let v = v.to_ascii_lowercase();
                v.contains("utf-8") || v.contains("utf8")
            })
            .unwrap_or(false)
    };
    if utf8("LC_ALL") || utf8("LC_CTYPE") || utf8("LANG") {
        return;
    }
    // SAFETY: called from the single-threaded endpoint entry points before
    // any thread is spawned.
    unsafe {
        if std::env::var_os("LC_ALL").is_some() {
            std::env::set_var("LC_ALL", "C.UTF-8");
        }
        std::env::set_var("LC_CTYPE", "C.UTF-8");
    }
}
