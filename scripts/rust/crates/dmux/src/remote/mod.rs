//! Remote owner-agent surface (plan §12). `protocol` carries the frozen
//! versioned message contract (§12.1). `agent` and `attach` are the two
//! binary entry points the root wired into hidden subcommands at the W5
//! dispatch (ADR 009); the remote/routing agent owns this module from that
//! record and fills in client, enrollment, and route logic around them.

pub mod agent;
pub mod attach;
pub mod protocol;
