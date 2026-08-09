pub mod cache;
pub mod config;
pub mod discovery;
pub mod docs;
pub mod engine;
pub mod help_parser;
pub mod history;
pub mod model;
pub mod native;
pub mod parser;
pub mod providers;
pub mod ranking;
pub mod safety;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
