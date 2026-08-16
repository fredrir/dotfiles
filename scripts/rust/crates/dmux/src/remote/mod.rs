//! Remote owner-agent surface. `protocol` carries the frozen versioned
//! message contract (plan §12.1); the SSH client implementation arrives in
//! P7 as `client.rs` under the remote/routing agent's ownership.

pub mod protocol;
