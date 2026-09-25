#![cfg_attr(not(target_os = "macos"), forbid(unsafe_code))]

pub mod cli;
pub mod collect;
pub mod formatting;
pub mod health;
pub mod identity;
pub mod inventory;
pub mod model;
pub mod presentation;
pub mod report;
pub mod top;

pub type Module = (&'static str, serde_json::Value);
