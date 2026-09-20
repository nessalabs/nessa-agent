//! Serializes an attached session's prompts, observations, and permission effects.
//!
//! ```text
//! sessions -> prompt_content (read a message's images; size its frame)
//!    |
//!    v
//! sessions -> worker -> application ExecutionController
//!               |  \-> tools / permissions wire mapping
//!               |  \-> event_queue -> session event reader
//!               v
//!            JSON-RPC
//! ```
//! Arrows show calls. The worker owns correlation and deadlines; the domain owns
//! state transitions. The event queue shares a decoded-byte budget across restored
//! generations and releases charges when ownership passes to the reader.
//! Startup notifications use the live session/profile verifier after context
//! admission. Earlier session updates fail closed because no admitted identity
//! can authorize them; configuration replies cannot conceal intervening drift.
//! Nested startup replies share the same absolute deadline as their pending RPC.
//! Live response writes and reads share the current operation deadline; shutdown
//! owns a separate, nonrenewing grace interval. Deadline regressions are grouped
//! in `worker/startup_responses.rs` and `worker/response_deadlines.rs` private tests.
//! Before prompt or native steering dispatch, ready provider messages pass through
//! the same policy verifier. Cooperative batches preserve close/deadline handling;
//! an uninterrupted ready stream delays dispatch until no messages are ready.
//! A partial frame waiting for more OS input does not block dispatch indefinitely;
//! buffered decode work and task-budget exhaustion do not count as absent input.
//! Permission audit delivery precedes successful settlement.
//! Failure aggregation retains primary operation errors alongside audit and
//! process-cleanup errors before settling command, stream, and close results.
//! The worker never awaits an injected image source: the session reads, verifies,
//! and encodes a message's images on the calling task, bounded and abandoned on
//! close, and hands the worker finished blocks. The worker keeps the check only it
//! can make for certain after a restoration, that the agent agreed to take images.
//! A frame's write deadline grows with the frame, so a message carrying images is
//! not failed by the one second meant for small frames.
//! Steering waits for a correlated acknowledgement before admitting another prompt;
//! an ambiguous result retires the worker and is never treated as unconsumed input.
pub(in crate::infrastructure::acp) mod event_queue;
mod failure;
pub(in crate::infrastructure::acp) mod prompt_content;
pub(in crate::infrastructure::acp) mod steering;
mod wire;
pub(crate) mod worker;
