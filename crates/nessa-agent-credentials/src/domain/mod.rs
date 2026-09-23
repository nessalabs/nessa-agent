//! Pure credential identities and values shared across process boundaries.
//!
//! Infrastructure maps these identities to environment variables or keychain
//! accounts. This module owns only which local agents accept a Nessa-managed
//! credential and which text is usable at both boundaries.

pub mod value_objects;
