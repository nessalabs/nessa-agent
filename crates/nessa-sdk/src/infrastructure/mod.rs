//! Infrastructure translates external representations into application input.
//! The JSON adapter handles parsing and read failures; it then calls the
//! application import path so domain construction still validates the model facts.
//!
//! ```text
//! host opens chosen file
//!         |
//!         v
//! reader --> JSON adapter --> application import --> domain constructors
//! ```
//! Arrows show the loading flow. The host chooses the file and loads it at startup.
//! The adapter does not choose a default path, discover models, or watch for edits.

pub mod model_metadata_json;
