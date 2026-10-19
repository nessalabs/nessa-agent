//! Fences a later restore must not undo: deleted conversations, revoked
//! share grants, and revoked credentials. Secret bytes are not a field.

use super::encoding::encode_identity as encode;

use super::record::Fingerprint;

/// The capabilities a revoked share grant held, the journal's `before`
/// column as [`crate::conversation::domain::Capabilities::as_str`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrantRole {
    Read,
    Comment,
    Drive,
}

impl GrantRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Comment => "read+comment",
            Self::Drive => "read+comment+drive",
        }
    }

    pub fn parse(token: &str) -> Option<Self> {
        match token {
            "read" => Some(Self::Read),
            "read+comment" => Some(Self::Comment),
            "read+comment+drive" => Some(Self::Drive),
            _ => None,
        }
    }
}

/// Whose credential was revoked.
///
/// An environment peer and a hosted worker share `ConsentClass::PeerEnvironment`
/// in the registry. That capture uses [`Self::EnvironmentPeer`]. [`Self::Worker`]
/// is for a fence that already knows the enrollment is a worker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialKind {
    Device,
    Peer,
    EnvironmentPeer,
    Worker,
}

impl CredentialKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Device => "device",
            Self::Peer => "peer",
            Self::EnvironmentPeer => "environment-peer",
            Self::Worker => "worker",
        }
    }

    pub fn parse(token: &str) -> Option<Self> {
        match token {
            "device" => Some(Self::Device),
            "peer" => Some(Self::Peer),
            "environment-peer" => Some(Self::EnvironmentPeer),
            "worker" => Some(Self::Worker),
            _ => None,
        }
    }

    /// `environment.serve` is the grant both an environment peer and a
    /// hosted worker hold. A reading `gateway:` principal is a peer.
    /// Anything else revoked in the registry is a device.
    pub fn from_registry(principal_id: &str, actions: &[String]) -> Self {
        if actions.iter().any(|action| action == "environment.serve") {
            Self::EnvironmentPeer
        } else if principal_id.starts_with("gateway:") {
            Self::Peer
        } else {
            Self::Device
        }
    }
}

/// What was fenced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FenceKind {
    Conversation {
        id: String,
    },
    ShareGrant {
        conversation_id: String,
        receiver_id: String,
        credential_id: String,
        role: GrantRole,
        sequence: u64,
    },
    Credential {
        kind: CredentialKind,
        credential_id: String,
        fingerprint: Fingerprint,
    },
}

/// One fence, keyed so capturing it again does not append a second row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeletionFence {
    key: String,
    kind: FenceKind,
}

impl DeletionFence {
    pub fn conversation(id: impl Into<String>) -> Result<Self, DeletionError> {
        let id = name(id.into())?;
        Ok(Self {
            key: format!("conversation:{id}"),
            kind: FenceKind::Conversation { id },
        })
    }

    pub fn share_grant(
        conversation_id: impl Into<String>,
        receiver_id: impl Into<String>,
        credential_id: impl Into<String>,
        role: GrantRole,
        sequence: u64,
    ) -> Result<Self, DeletionError> {
        let conversation_id = name(conversation_id.into())?;
        let receiver_id = name(receiver_id.into())?;
        let credential_id = name(credential_id.into())?;
        Ok(Self {
            key: format!("grant:{conversation_id}:{receiver_id}:{sequence}"),
            kind: FenceKind::ShareGrant {
                conversation_id,
                receiver_id,
                credential_id,
                role,
                sequence,
            },
        })
    }

    /// `fingerprint` is a hash. The credential's verifier and private key
    /// are not parameters of this constructor.
    pub fn credential(
        kind: CredentialKind,
        credential_id: impl Into<String>,
        fingerprint: Fingerprint,
    ) -> Result<Self, DeletionError> {
        let credential_id = name(credential_id.into())?;
        Ok(Self {
            key: format!("credential:{}:{credential_id}", kind.as_str()),
            kind: FenceKind::Credential {
                kind,
                credential_id,
                fingerprint,
            },
        })
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn kind(&self) -> &FenceKind {
        &self.kind
    }
}

/// One appended fence, with the sequence it received.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeletionEntry {
    sequence: u64,
    fence: DeletionFence,
}

impl DeletionEntry {
    /// `sequence` is the place `fence` already occupies in a log that
    /// [`DeletionLog::new`] will check.
    pub(crate) fn from_persisted(
        sequence: u64,
        fence: DeletionFence,
    ) -> Result<Self, DeletionError> {
        if sequence == 0 {
            return Err(DeletionError::Sequence);
        }
        Ok(Self { sequence, fence })
    }

    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn fence(&self) -> &DeletionFence {
        &self.fence
    }

    pub fn canonical(&self) -> String {
        match self.fence.kind() {
            FenceKind::Conversation { id } => {
                format!("{} conversation {}\n", self.sequence, encode(id))
            }
            FenceKind::ShareGrant {
                conversation_id,
                receiver_id,
                credential_id,
                role,
                sequence,
            } => format!(
                "{} grant {} {} {} {} {sequence}\n",
                self.sequence,
                encode(conversation_id),
                encode(receiver_id),
                encode(credential_id),
                role.as_str()
            ),
            FenceKind::Credential {
                kind,
                credential_id,
                fingerprint,
            } => format!(
                "{} credential {} {} {}\n",
                self.sequence,
                kind.as_str(),
                encode(credential_id),
                fingerprint.hex()
            ),
        }
    }
}

/// The inventory as a value: sequences start at 1 and only increase.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeletionLog {
    entries: Vec<DeletionEntry>,
}

impl DeletionLog {
    pub fn new(entries: Vec<DeletionEntry>) -> Result<Self, DeletionError> {
        let mut previous = 0u64;
        for entry in &entries {
            if entry.sequence != previous + 1 {
                return Err(DeletionError::Sequence);
            }
            previous = entry.sequence;
        }
        Ok(Self { entries })
    }

    pub fn entries(&self) -> &[DeletionEntry] {
        &self.entries
    }

    /// Append `fence` at the next sequence, or return the sequence it
    /// already has. The returned log is a new value.
    pub fn append(&self, fence: DeletionFence) -> (Self, Append) {
        if let Some(existing) = self
            .entries
            .iter()
            .find(|entry| entry.fence.key == fence.key)
        {
            return (
                self.clone(),
                Append::AlreadyRecorded {
                    sequence: existing.sequence,
                },
            );
        }
        let sequence = self.entries.len() as u64 + 1;
        let mut entries = self.entries.clone();
        entries.push(DeletionEntry { sequence, fence });
        (Self { entries }, Append::Recorded { sequence })
    }

    pub fn canonical(&self) -> String {
        let mut text = String::from("nessa-deletion-inventory/1\n");
        for entry in &self.entries {
            text.push_str(&entry.canonical());
        }
        text
    }

    pub fn watermark(&self) -> String {
        self.entries.len().to_string()
    }
}

/// What an append did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Append {
    Recorded { sequence: u64 },
    AlreadyRecorded { sequence: u64 },
}

/// Why a fence or a log was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeletionError {
    /// A name was empty, too long, or contained a control character.
    Name,
    /// Sequences were not 1, 2, 3, … without gaps.
    Sequence,
}

fn name(value: String) -> Result<String, DeletionError> {
    if value.is_empty() || value.len() > 512 || value.bytes().any(|byte| byte < 0x20) {
        return Err(DeletionError::Name);
    }
    Ok(value)
}
