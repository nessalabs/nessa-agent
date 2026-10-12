//! The choice to let Claude or Codex run its own hooks, and the evidence of
//! changing it.
//!
//! ```text
//! stored bytes -> selection_from_stored -> NativeHooksSelection
//! change -> requested evidence -> file -> outcome evidence
//! ```
//!
//! Arrows are the read and the write. Invalid stored bytes are skipped and
//! read as off. OpenCode is not a choice here. The file and the audit adapter
//! live in infrastructure; this module owns the selection and the evidence
//! the adapter is given.

use serde::de::{self, Deserializer as _, IgnoredAny, MapAccess, Visitor};
use serde_json::Value;
use std::fmt;

/// A harness whose own hooks the caller can allow.
///
/// OpenCode is absent on purpose. Its launch stays suppressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeHooksAgent {
    /// Claude Code.
    Claude,
    /// Codex.
    Codex,
}

impl NativeHooksAgent {
    /// The agent name on the wire and in the stored file.
    pub fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}

/// What the next launch of each opted-in harness will do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeHooksSelection {
    claude: bool,
    codex: bool,
}

impl NativeHooksSelection {
    /// A choice for each harness. True omits that harness's suppression request.
    pub fn new(claude: bool, codex: bool) -> Self {
        Self { claude, codex }
    }

    /// Both harnesses keep today's suppression.
    pub fn off() -> Self {
        Self::new(false, false)
    }

    /// Whether Claude omits its suppression request.
    pub fn claude(self) -> bool {
        self.claude
    }

    /// Whether Codex omits its suppression request.
    pub fn codex(self) -> bool {
        self.codex
    }

    /// The choice for one harness.
    pub fn get(self, agent: NativeHooksAgent) -> bool {
        match agent {
            NativeHooksAgent::Claude => self.claude,
            NativeHooksAgent::Codex => self.codex,
        }
    }

    /// A selection with one harness changed.
    pub fn with(self, agent: NativeHooksAgent, enabled: bool) -> Self {
        match agent {
            NativeHooksAgent::Claude => Self::new(enabled, self.codex),
            NativeHooksAgent::Codex => Self::new(self.claude, enabled),
        }
    }
}

/// Read a stored document. Anything that is not a boolean under `claude` or
/// `codex` is off. Unknown keys are ignored. A document that is not an object,
/// is not JSON, or names `claude` or `codex` twice is off: a repeated key has
/// no single answer, and a parser that keeps the last one could turn hooks on.
pub fn selection_from_stored(bytes: &[u8]) -> NativeHooksSelection {
    let mut reader = serde_json::Deserializer::from_slice(bytes);
    match reader
        .deserialize_map(StoredChoice)
        .and_then(|selection| reader.end().map(|()| selection))
    {
        Ok(selection) => selection,
        Err(_) => NativeHooksSelection::off(),
    }
}

/// Reads the stored object one member at a time, so a repeated harness key
/// is seen rather than overwritten.
struct StoredChoice;

impl<'de> Visitor<'de> for StoredChoice {
    type Value = NativeHooksSelection;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("an object of native hooks choices")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut members: A) -> Result<Self::Value, A::Error> {
        let mut claude = None;
        let mut codex = None;
        while let Some(key) = members.next_key::<String>()? {
            let slot = if key == NativeHooksAgent::Claude.name() {
                &mut claude
            } else if key == NativeHooksAgent::Codex.name() {
                &mut codex
            } else {
                members.next_value::<IgnoredAny>()?;
                continue;
            };
            if slot.is_some() {
                return Err(de::Error::custom(format_args!("duplicate `{key}`")));
            }
            *slot = Some(members.next_value::<Value>()?.as_bool().unwrap_or(false));
        }
        Ok(NativeHooksSelection::new(
            claude.unwrap_or(false),
            codex.unwrap_or(false),
        ))
    }
}

/// Where a settings change learns whether the caller is a person, an agent,
/// an integration, or a gateway. Missing and unreadable principals are not a
/// kind this port invents.
pub trait CallerKindSource: Send + Sync {
    /// The audit word for `principal_id`, or `"unknown"` when this source
    /// cannot say.
    fn caller_kind(&self, principal_id: &str) -> &'static str;
}

/// Which evidence record this is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeHooksPhase {
    /// Written before the file changes.
    Requested,
    /// Written after the file matches the request, or after a no-op.
    Outcome,
}

impl NativeHooksPhase {
    /// The phase name stored in the record.
    pub fn name(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Outcome => "outcome",
        }
    }
}

/// What a caller asked, and what was true before the ask.
///
/// The durable adapter assigns the record id and the observation time. `after`
/// is present only on an outcome record, and it is the value in effect once
/// that record is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeHooksEvidence {
    /// The harness this record is about.
    pub agent: NativeHooksAgent,
    /// Requested, then outcome.
    pub phase: NativeHooksPhase,
    /// The choice in effect before this command.
    pub before: bool,
    /// The choice the caller asked for.
    pub requested: bool,
    /// The choice in effect when the outcome record is written.
    pub after: Option<bool>,
    /// The caller's stable id for this command. A retry uses the same id.
    pub request_id: String,
    /// The caller's organization.
    pub organization_id: String,
    /// The caller's principal.
    pub principal_id: String,
    /// The credential the caller presented. This is the surface.
    pub surface_id: String,
    /// `person`, `agent`, `integration`, `gateway`, or `unknown`.
    pub caller_kind: String,
}

/// One command to change one harness.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeHooksChange {
    /// Claude or Codex.
    pub agent: NativeHooksAgent,
    /// True omits suppression on the next launch.
    pub enabled: bool,
    /// Stable id for this command. A retry reuses it.
    pub request_id: String,
    /// Organization of the caller.
    pub organization_id: String,
    /// Principal of the caller.
    pub principal_id: String,
    /// Credential id of the caller.
    pub surface_id: String,
    /// `person`, `agent`, `integration`, `gateway`, or `unknown`.
    pub caller_kind: String,
}

impl NativeHooksChange {
    /// The evidence for one phase of this command.
    pub fn evidence(
        &self,
        phase: NativeHooksPhase,
        before: bool,
        after: Option<bool>,
    ) -> NativeHooksEvidence {
        NativeHooksEvidence {
            agent: self.agent,
            phase,
            before,
            requested: self.enabled,
            after,
            request_id: self.request_id.clone(),
            organization_id: self.organization_id.clone(),
            principal_id: self.principal_id.clone(),
            surface_id: self.surface_id.clone(),
            caller_kind: self.caller_kind.clone(),
        }
    }
}

/// The port a settings change records through before it reports success.
pub trait NativeHooksAudit: Send + Sync {
    /// Record one phase. Failure must stop the command from reporting success.
    fn record(&self, evidence: &NativeHooksEvidence) -> Result<(), NativeHooksError>;
}

/// Why a change was not committed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeHooksError {
    /// The audit record was not written. The choice was not left changed
    /// unless reverting the file also failed; either way the command fails.
    Audit,
    /// The choice file could not be written. The running switch is unchanged
    /// when this is returned from the write that would have applied it.
    Storage,
    /// The blocking save did not finish: its task panicked or was cancelled.
    /// That is not an audit failure, and it is not a failed write of the
    /// choice file. Whether the switch moved is whatever the task did before
    /// it stopped.
    Interrupted,
}

impl std::fmt::Display for NativeHooksError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Audit => "native hooks audit failed",
            Self::Storage => "native hooks choice could not be stored",
            Self::Interrupted => "native hooks save did not finish",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_stored_choices_are_off() {
        assert_eq!(selection_from_stored(b""), NativeHooksSelection::off());
        assert_eq!(selection_from_stored(b"[]"), NativeHooksSelection::off());
        assert_eq!(
            selection_from_stored(br#"{"claude":"yes","codex":1,"opencode":true,"other":false}"#),
            NativeHooksSelection::off()
        );
        assert_eq!(
            selection_from_stored(br#"{"claude":true,"codex":false}"#),
            NativeHooksSelection::new(true, false)
        );
    }

    #[test]
    fn a_harness_named_twice_is_off_whichever_value_comes_last() {
        assert_eq!(
            selection_from_stored(br#"{"claude":false,"claude":true,"codex":true}"#),
            NativeHooksSelection::off()
        );
        assert_eq!(
            selection_from_stored(br#"{"codex":true,"codex":true}"#),
            NativeHooksSelection::off()
        );
        // A repeated key that is not a harness changes nothing.
        assert_eq!(
            selection_from_stored(br#"{"other":1,"other":2,"codex":true}"#),
            NativeHooksSelection::new(false, true)
        );
    }
}
