//! A local passphrase vault, independent of any provider or desktop framework.
//! `application` owns session lifetime and entry access through injected ports;
//! `infrastructure` seals private files with Argon2id and authenticated encryption.
//! No operation returns the vault key or exposes stored entries over a wire API.
pub mod application;
pub mod domain;
pub mod infrastructure;
/// Published bounds shared by native code and UI from one checked-in contract.
pub mod contract {
    include!(concat!(env!("OUT_DIR"), "/vault_contract.rs"));
}
