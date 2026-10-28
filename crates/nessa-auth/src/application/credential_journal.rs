//! Shared journal identity decoding for actual credential lifecycle facts.
use super::{dto::CredentialTransitionDto, ports::AccessError};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct JournalIdentity {
    sequence: u64,
    revision: u64,
    correlation: Option<Box<str>>,
}
impl JournalIdentity {
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn correlation(&self) -> Option<&str> {
        self.correlation.as_deref()
    }
    pub fn take(record: &mut CredentialTransitionDto) -> Result<Self, AccessError> {
        let correlation = record.correlation.take();
        if record.sequence == 0
            || record.revision == 0
            || correlation
                .as_ref()
                .is_some_and(|id| id.trim().is_empty() || id.len() > 200)
        {
            return Err(AccessError::Unavailable);
        }
        Ok(Self {
            sequence: record.sequence,
            revision: record.revision,
            correlation: correlation.map(String::into_boxed_str),
        })
    }
}
