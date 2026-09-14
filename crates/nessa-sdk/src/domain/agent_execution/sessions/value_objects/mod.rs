//! Session identities and immutable evidence of execution release and attachment closure.
//!
//! ```text
//! validated text --> ExecutionSessionId --> ExecutionSession --> SessionClosure
//!                                                       \--> ExecutionFinish
//! ```
//!
//! Arrows mean identity construction, aggregate ownership, and once-only evidence emission.
mod identity;
pub use identity::{ExecutionSessionId, SessionId};

mod closure;
pub use closure::SessionClosure;

mod finish;
pub use finish::ExecutionFinish;
