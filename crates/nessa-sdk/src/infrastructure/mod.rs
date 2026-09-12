//! Infrastructure translates external representations into application input and
//! implements the application's execution ports.
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
//! The JSON adapter does not choose a default path, discover models, or watch edits.
//! Claude ACP separately translates bounded process I/O into application events;
//! host composition supplies its exact model, launch configuration, and limits.

pub mod claude_acp;
pub mod model_metadata_json;
