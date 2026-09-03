//! Connect RPC handler — orchestrates middleware then success response.
//!
//! `handler::handle_connect` is the only public entry for the `connect` method.
//! It never parses JSON; it receives `ConnectParams` from `decode_client_request`.

pub mod handler;
