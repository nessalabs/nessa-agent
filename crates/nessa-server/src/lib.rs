//! Nessa gateway with mandatory credential authentication and per-operation authorization.
pub mod app;
pub mod composition;
pub mod core;
pub mod env;
pub mod health;
pub mod product;
pub mod protocol;
pub mod server;

pub use core::run;
