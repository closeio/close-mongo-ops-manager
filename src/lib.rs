//! Close MongoDB Operations Manager: a terminal UI to monitor and kill MongoDB
//! operations.

pub mod app;
pub mod cli;
pub mod config;
pub mod error;
pub mod logging;
pub mod model;
pub mod mongo;
pub mod runtime;
pub mod theme;
pub mod ui;

#[cfg(test)]
pub(crate) mod testutil;
