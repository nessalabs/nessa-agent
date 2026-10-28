//! What a GitHub sign-in is, without the network, the store or the clock.

mod repository;
pub use repository::{GithubReadInputError, GithubReadKind, GithubRepository};

/// The GitHub account a sign-in belongs to: its numeric id, which never
/// changes, and its login, which can.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GithubAccount {
    pub id: u64,
    pub login: String,
}

/// A user access token for the Nessa GitHub App. `Debug` does not print it.
#[derive(Clone, PartialEq, Eq)]
pub struct AccessToken(String);

impl AccessToken {
    /// `None` for an empty token or one holding a control character, which
    /// no header or environment variable could carry.
    pub fn new(token: String) -> Option<Self> {
        (!token.is_empty() && !token.chars().any(char::is_control)).then_some(Self(token))
    }
    /// The token itself, for the one header or variable it is sent in.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for AccessToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AccessToken(..)")
    }
}

/// A signed-in account and its token: what is stored, and what agents are
/// given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GithubSignIn {
    pub account: GithubAccount,
    pub token: AccessToken,
}

/// Why a sign-in that was started did not end signed in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectEnd {
    /// The code's time ran out before the person approved it.
    Expired,
    /// The person refused it on GitHub.
    Denied,
    /// The person, or another window, cancelled it here.
    Cancelled,
    /// GitHub refused the app itself: the client id is not one it knows, or
    /// the app does not allow the device flow.
    Rejected,
    /// GitHub could not be reached to ask for a code.
    Unreachable,
    /// The token could not be kept, so it was dropped.
    StoreUnavailable,
    /// A record of the sign-in could not be kept, so it was dropped.
    AuditUnavailable,
}

/// Where the sign-in stands, as the window is told it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GithubStatus {
    /// Not signed in. `last` is why the latest attempt ended, until another
    /// starts; `None` when there was none or it was disconnected.
    /// `unrecorded` when that end (or a sign-out) could not be kept in the
    /// audit: what happened stands, and the missing record is said beside it.
    SignedOut {
        last: Option<ConnectEnd>,
        unrecorded: bool,
    },
    /// Waiting for the person to enter `user_code` at `verification_uri`.
    Connecting {
        user_code: String,
        verification_uri: String,
        /// Whole seconds left before the code expires, at the moment asked.
        expires_in_seconds: u64,
    },
    /// Signed in as `account`.
    /// Signed in. `unrecorded` when a sign-out failed and that failure could
    /// not be kept in the audit either.
    Connected {
        account: GithubAccount,
        unrecorded: bool,
    },
}
