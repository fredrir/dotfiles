#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "linux")]
pub use linux::{available, hold, is_holder, read, read_for_peer, write};
#[cfg(target_os = "macos")]
pub use macos::{available, read, read_for_peer, write};
