//! One-time preparation of the agent runtime, off the request path.
//!
//! The first launch of a freshly installed or updated runtime is scanned by the
//! operating system before it can run. Paying for that inside a user's first
//! message is what made the first message fail; this context pays for it once,
//! in the background, as soon as the gateway is listening with an agent
//! configured.
//!
//! ```text
//! composition -> AgentWarmUp::start   (one self-owned background run)
//!                    |
//!                    |-> AgentProvider  open -> close   (a real session)
//!                    |-> WarmUpRecords  completion in the data directory
//!                    |-> WarmUpAudit    evidence for that transition
//!                    |
//! ConversationService -> RuntimeReadiness::wait ---^  (joins the same run)
//! ```
//!
//! Arrows show calls. The conversation service waits on the run already in
//! flight rather than starting a second cold launch. The terminal keeps the
//! preparation result, physical ownership, audit delivery and completion-record
//! delivery separate. Composition may retry a released failure, while uncertain
//! physical ownership remains fenced. A warm-up failure never becomes the
//! conversation result: its freshly resolved provider reports its own outcome.
pub mod application;
pub mod domain;
pub mod infrastructure;
