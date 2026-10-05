//! Native pairing as both ends of the connection speak it: length-prefixed
//! framing, the enrollment envelope codec, the enrollment channel over Auth's
//! TLS transport, the protected product frame bounds, and the
//! deadline-and-wake socket a connection runs on.
//!
//! ```text
//! enrollment_channel --> wire (envelope codec) --> status (what a status reply says)
//!                    --> frames (length-prefixed framing)
//!                    --> Auth NativeTransport
//! socket (deadline_stream --> wake, Clock; worker)
//! limits --> protocol / product bounds
//! ```
//! Arrows are compile-time dependencies. The gateway's connection worker and
//! the device's enrollment client both run on `socket` and `enrollment_channel`;
//! neither end keeps a copy. Framing offsets are IO progress, not enrollment
//! state, and nothing here decides enrollment: Auth owns every phase.
mod enrollment_channel;
mod frames;
mod limits;
pub mod socket;
mod status;
pub mod wire;
pub use enrollment_channel::{EnrollmentChannel, NativeFrameError};
pub use frames::{encode_frame, FrameReader, FrameTooLarge};
pub use limits::{MAX_PROTECTED_REQUEST_BYTES, MAX_PROTECTED_RESPONSE_BYTES};
pub use status::DevicePairingStatus;
