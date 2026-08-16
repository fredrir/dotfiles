//! P7 gate tests (plan §18 P7, §20.2 cases 16–22 slice owned here):
//! the versioned `_agent` endpoint driven through the REAL built binary
//! over the direct-argv transport, the `_attach` single-use-token PTY
//! channel, the route retry matrix over fake transports, and the env-gated
//! two-host (Archie) leg over real ssh.
//!
//! Every registry lives in scratch tempdirs (`--data-dir`/`--lock-dir`
//! seams); every tmux server is a scratch `-L` namespace killed on drop.
//! No test touches a production registry, socket, or the default tmux
//! server — locally or on Archie.

mod util;

mod attach;
mod local_agent;
mod mutations;
mod route_matrix;
mod two_host;
