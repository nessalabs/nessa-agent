//! One-time preparation of the agent runtime, off the request path.
//!
//! The first launch of a freshly installed or updated runtime is scanned by the
//! operating system before it can run. Paying for that inside a user's first
//! message is what made the first message fail; this context pays for it once,
//! in the background, as soon as the gateway is listening with an agent
//! configured.
//!
//! ```text
//! composition -> AgentWarmUp::start   (background task, once per fingerprint)
//!                    |
//!                    |-> AgentProvider  open -> close   (a real session)
//!                    |-> WarmUpRecords  completion in the data directory
//!                    |-> WarmUpAudit    evidence for that transition
//!                    |
//! ConversationService -> RuntimeReadiness::wait ---^  (joins the same run)
//! ```
//!
//! Arrows show calls. The conversation service waits on the run already in
//! flight rather than starting a second cold launch. It never fails because the
//! warm-up failed: a failed preparation is not evidence that the user's own
//! launch will fail, so the request opens its own provider and reports its own
//! outcome.
pub mod application;
pub mod domain;
pub mod infrastructure;
