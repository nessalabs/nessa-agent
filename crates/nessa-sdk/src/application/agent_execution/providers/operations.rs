#![deny(missing_docs)]

/// Provider operations available to an Agent, separate from model modalities and
/// token limits. This is a point-in-time support snapshot, not an admission permit:
/// an operation can still fail because the session is busy, closed, or unavailable.
///
/// ACP reports the latest successfully negotiated connection. Restoration clears
/// the snapshot while reconnecting and publishes new support after validation.
/// Closing retains the last negotiation; it does not promise restoration success.
/// Custom backends default to no advertised support and opt in explicitly.
///
/// A cleared snapshot and a negotiated "no" both read `false` in the support
/// fields, and they do not mean the same thing: one is not yet known, the other
/// is the agent's answer. [`Self::negotiated`] tells them apart.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OperationCapabilities {
    /// The other fields are the answer of a completed negotiation with the
    /// provider. False while a connection is being opened or restored, and for
    /// a backend that negotiates nothing: then a `false` beside it means "not
    /// known", never "no". A closed context keeps its last negotiation, so this
    /// stays true across close.
    ///
    /// Admission refuses an image message only on a known "no": this true and
    /// [`Self::image_input`] false. While the answer is unknown the message is
    /// admitted, and the adapter refuses it at dispatch, with the same typed
    /// error, if the restored agent turns out not to take images.
    pub negotiated: bool,
    /// The provider can inject input into an identified active invocation.
    /// This does not describe SDK-owned queueing or next-invocation steering.
    pub native_steering: bool,
    /// The provider supports restoring the same context after connection close.
    /// Saved history can still be missing or unavailable when restoration is tried.
    pub session_resume: bool,
    /// A user message's images can be carried on this connection: the connected
    /// agent agreed to receive them *and* this process can supply the bytes.
    /// Model metadata says what a model can see; this says what this connection
    /// will actually deliver, so an agent that agreed still reports false where
    /// composition gave the binding no image source to read from.
    pub image_input: bool,
}
