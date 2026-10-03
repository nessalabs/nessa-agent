//! Native enrollment over TLS and PAKE: the gateway listener and runtime, the
//! device client, framing, and the bounded blocking workers they run on.
//!
//! ```text
//! listener --> connection --> runtime --> application (owner, read_status) --> Auth
//! client   --> enrollment_channel --> wire + Auth NativeTransport
//! connection --> enrollment_channel
//! runtime  --> registration (one code registration at a time)
//! owner_commands --> runtime (owner side only; the product socket's handle)
//! receivers --> conversation receiver authority (application's PairingReceivers)
//! ```
//! Arrows are compile-time dependencies. Auth owns every enrollment phase; the
//! runtime holds only the volatile PAKE setup of the one open invitation.
//! Framing offsets are IO progress, not enrollment state. `shutdown` on each
//! owner stops admission, wakes blocked sockets, and waits for its workers.
mod client;
mod connection;
mod enrollment_channel;
mod identity;
mod listener;
mod owner_commands;
mod receivers;
mod registration;
mod runtime;
pub mod wire;
mod worker;
pub use client::{NativeClientError, NativeEnrollmentClient, NativeRetryOutcome};
pub use connection::{
    wake::{NativeWakeCause, NativeWakeOutcome, NativeWakeReport},
    NativeConnectionError, NativeConnectionFailure, NativeEnrollmentConnections,
};
pub use enrollment_channel::{EnrollmentChannel, NativeFrameError};
pub use identity::{restore_gateway_identity, GatewayIdentityError};
pub use listener::{Accepted, EnrollmentAccept, NativeEnrollmentListener, TcpEnrollmentAccept};
pub use owner_commands::{InvitationEntropy, InvitationEntropySource, PairingOwnerCommands};
pub use receivers::ConversationReceivers;
pub use registration::{RegisteredInvitation, RegistrationError, RegistrationWorker};
pub use runtime::{
    BeginPairing, CreatedInvitation, GatewayPairing, PairingRuntimeDependencies,
    PairingRuntimeError, ServerHandshake,
};
