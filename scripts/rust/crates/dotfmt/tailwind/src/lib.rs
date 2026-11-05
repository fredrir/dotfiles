#![forbid(unsafe_code)]

pub mod bridge;
pub mod config;
pub mod extract;
mod merge;
pub mod order;
pub mod sorter;

pub use config::{Config, Mode, OrderSource};
pub use order::{OrderTable, Sorter};
pub use sorter::{sort_class_list, split_classes};
