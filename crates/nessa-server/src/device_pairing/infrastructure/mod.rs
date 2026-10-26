//! Native enrollment over TLS and PAKE: the gateway listener and runtime, the
//! bounded blocking workers they run on. The device client is owned by
//! `nessa_client_core::pairing`, which a peer gateway's side and the gateway
//! tests consume. A connection
//! whose first envelope is `openProduct` becomes a protected product session,
//! moving to the product session pool and returning its connection permit.
//! Framing, the envelope codec, the enrollment channel and the deadline socket
//! both ends run on are `nessa_protocol::pairing`.
//!
//! ```text
//! listener --> connection --> runtime --> application (owner, read_status) --> Auth
//! connection --> nessa_protocol::pairing (enrollment_channel, wire, socket)
//! connection --> protected (framed tls_stream) --> ProtectedSessions (product)
//! connection --> tls_stream --> EnvironmentPeers (paired environments)
//! runtime  --> registration (one code registration at a time)
//! linking_audit --> application's LinkingAudit (pairing.setLinking's records)
//! announcer --> application's LinkingAnnouncer (multicast DNS-SD while listening on the network)
//! owner_commands --> runtime (owner side only; the product socket's handle)
//! runtime --> FenceCapture (after pairing.cancel commits; a failure leaves it)
//! receivers --> conversation receiver authority (application's PairingReceivers)
//! runtime  --> owner_admission (every owner command's lease; drained at shutdown)
//! ```
//! Arrows are compile-time dependencies. Auth owns every enrollment phase; the
//! runtime holds only the volatile PAKE setup of the one open invitation.
//! Framing offsets are IO progress, not enrollment state. `shutdown` on each
//! owner stops admission, wakes blocked sockets, and waits for its workers.
mod announcer;
mod connection;
mod environment_peers;
mod identity;
#[cfg(unix)]
mod linking_audit;
mod listener;
mod owner_admission;
mod owner_commands;
mod protected;
mod receivers;
mod registration;
mod runtime;
mod tls_stream;
pub use announcer::{MulticastAnnouncer, GATEWAY_KEY, LINKING_SERVICE};
pub(crate) use connection::RELAY_CAPACITY;
pub use connection::{
    NativeConnectionError, NativeConnectionFailure, NativeEnrollmentConnections, RelayPermit,
};
pub use environment_peers::EnvironmentPeers;
pub use identity::{restore_gateway_identity, GatewayIdentityError};
#[cfg(unix)]
pub use linking_audit::DurableLinkingAudit;
pub use listener::{Accepted, EnrollmentAccept, NativeEnrollmentListener, TcpEnrollmentAccept};
pub use owner_commands::{InvitationEntropy, InvitationEntropySource, PairingOwnerCommands};
pub use protected::{ProtectedConnection, ProtectedSessions};
pub use receivers::ConversationReceivers;
pub use registration::{RegisteredInvitation, RegistrationError, RegistrationWorker};
pub use runtime::{
    BeginPairing, CreatedInvitation, GatewayPairing, PairingRuntimeDependencies,
    PairingRuntimeError, PhoneAuthorization, ServerHandshake,
};
pub use tls_stream::{TlsCloser, TlsStream};
