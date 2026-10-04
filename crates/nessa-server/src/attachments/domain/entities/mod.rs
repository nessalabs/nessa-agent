//! Identity-bearing records: a hold is one conversation keeping one stored file.
//! `RetiredFrom` constrains the existing predecessor shared by retirement
//! ports, saved records and reversal audit.
mod hold;

pub use hold::{Hold, HoldState, RetiredFrom};
