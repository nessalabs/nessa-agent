use std::{io::Write, path::PathBuf};

use nessa_local_storage::{
    create_private_directory_tree_beneath, sync_directory_beneath, PrivateTempFile,
};
use serde::Serialize;

use crate::agent_credentials::{
    application::{CredentialSaveAudit, CredentialSaveAuditFailure, CredentialSaveIds},
    domain::value_objects::{
        CredentialSaveCaller, CredentialSaveCorrelation, CredentialSaveEffect,
        CredentialSaveIntent, CredentialSaveOutcome, CredentialSaveRefusal,
        CredentialSaveUncertainty,
    },
};

/// Operating-system correlations for independently settling save work.
pub struct RandomCredentialSaveIds;

/// Audit sink used when desktop composition cannot resolve a trusted root.
pub struct UnavailableCredentialSaveAudit;

impl CredentialSaveAudit for UnavailableCredentialSaveAudit {
    fn record_intent(&self, _: &CredentialSaveIntent) -> Result<(), CredentialSaveAuditFailure> {
        Err(CredentialSaveAuditFailure)
    }

    fn record_outcome(&self, _: &CredentialSaveOutcome) -> Result<(), CredentialSaveAuditFailure> {
        Err(CredentialSaveAuditFailure)
    }
}

impl CredentialSaveIds for RandomCredentialSaveIds {
    fn next(&self) -> Result<CredentialSaveCorrelation, CredentialSaveAuditFailure> {
        let mut bytes = [0_u8; 16];
        getrandom::fill(&mut bytes).map_err(|_| CredentialSaveAuditFailure)?;
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        let value = format!(
            "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
        );
        CredentialSaveCorrelation::parse(value).map_err(|_| CredentialSaveAuditFailure)
    }
}

/// Immutable local audit records for credential replacement intent and outcome.
pub struct FileCredentialSaveAudit {
    trusted_root: PathBuf,
    relative_directory: PathBuf,
}

impl FileCredentialSaveAudit {
    pub fn beneath(trusted_root: PathBuf, relative_directory: PathBuf) -> Self {
        Self {
            trusted_root,
            relative_directory,
        }
    }

    fn publish<T: Serialize>(
        &self,
        correlation: &CredentialSaveCorrelation,
        suffix: &str,
        record: &T,
    ) -> Result<(), CredentialSaveAuditFailure> {
        create_private_directory_tree_beneath(&self.trusted_root, &self.relative_directory)
            .map_err(|_| CredentialSaveAuditFailure)?;
        let destination = self
            .relative_directory
            .join(format!("{}-{suffix}.json", correlation.as_str()));
        let bytes = serde_json::to_vec(record).map_err(|_| CredentialSaveAuditFailure)?;
        let mut temporary =
            PrivateTempFile::new_beneath(&self.trusted_root, &self.relative_directory)
                .map_err(|_| CredentialSaveAuditFailure)?;
        temporary
            .as_file_mut()
            .write_all(&bytes)
            .and_then(|()| temporary.as_file().sync_all())
            .map_err(|_| CredentialSaveAuditFailure)?;
        temporary
            .publish_new_beneath(&destination)
            .and_then(|()| sync_directory_beneath(&self.trusted_root, &self.relative_directory))
            .map_err(|_| CredentialSaveAuditFailure)
    }
}

impl CredentialSaveAudit for FileCredentialSaveAudit {
    fn record_intent(
        &self,
        intent: &CredentialSaveIntent,
    ) -> Result<(), CredentialSaveAuditFailure> {
        self.publish(intent.correlation(), "intent", &IntentRecord::from(intent))
    }

    fn record_outcome(
        &self,
        outcome: &CredentialSaveOutcome,
    ) -> Result<(), CredentialSaveAuditFailure> {
        self.publish(
            outcome.intent().correlation(),
            "outcome",
            &OutcomeRecord::from(outcome),
        )
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct IntentRecord<'a> {
    correlation: &'a str,
    caller: &'static str,
    agent: &'static str,
    stage: &'a str,
    instance: Option<&'a str>,
    service: &'a str,
    account: &'a str,
    before: &'static str,
    intended: &'static str,
}

impl<'a> From<&'a CredentialSaveIntent> for IntentRecord<'a> {
    fn from(intent: &'a CredentialSaveIntent) -> Self {
        Self {
            correlation: intent.correlation().as_str(),
            caller: caller_name(intent.caller()),
            agent: agent_name(intent.agent()),
            stage: intent.namespace().stage(),
            instance: intent.namespace().instance(),
            service: intent.target().service(),
            account: intent.target().account(),
            before: "replacement-requested-prior-secret-not-inspected",
            intended: "replace-api-key",
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OutcomeRecord<'a> {
    correlation: &'a str,
    effect: &'static str,
}

impl<'a> From<&'a CredentialSaveOutcome> for OutcomeRecord<'a> {
    fn from(outcome: &'a CredentialSaveOutcome) -> Self {
        Self {
            correlation: outcome.intent().correlation().as_str(),
            effect: match outcome.effect() {
                CredentialSaveEffect::Confirmed => "confirmed",
                CredentialSaveEffect::Refused(CredentialSaveRefusal::Invalid) => "refused-invalid",
                CredentialSaveEffect::Uncertain(CredentialSaveUncertainty::StoreUnavailable) => {
                    "uncertain-store-unavailable"
                }
                CredentialSaveEffect::Uncertain(CredentialSaveUncertainty::StorePanicked) => {
                    "uncertain-store-panicked"
                }
            },
        }
    }
}

fn caller_name(caller: CredentialSaveCaller) -> &'static str {
    match caller {
        CredentialSaveCaller::Main => "main",
        CredentialSaveCaller::Setup => "setup",
    }
}

fn agent_name(agent: nessa_agent_credentials::CredentialAgent) -> &'static str {
    match agent {
        nessa_agent_credentials::CredentialAgent::Claude => "claude",
        nessa_agent_credentials::CredentialAgent::Opencode => "opencode",
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    use nessa_agent_credentials::{CredentialAgent, CredentialNamespace};

    use crate::agent_credentials::domain::value_objects::{
        CredentialSaveTarget, CredentialSaveTargetError,
    };

    use super::*;

    fn intent() -> Result<CredentialSaveIntent, CredentialSaveTargetError> {
        let namespace = CredentialNamespace::new("ci".into(), Some("one".into())).unwrap();
        let target = CredentialSaveTarget::new(
            CredentialAgent::Claude,
            namespace,
            "so.nessa.agent-credentials".into(),
            "ci:one:claude-api-key".into(),
        )?;
        Ok(CredentialSaveIntent::new(
            CredentialSaveCorrelation::parse("550e8400-e29b-41d4-a716-446655440000".into())
                .unwrap(),
            CredentialSaveCaller::Setup,
            target,
        ))
    }

    #[test]
    fn records_exact_target_and_never_replace_existing_evidence() {
        let root = tempfile::tempdir().unwrap();
        let trusted_root = root.path().join("private");
        nessa_local_storage::create_directory(&trusted_root).unwrap();
        let audit = FileCredentialSaveAudit::beneath(
            trusted_root.clone(),
            PathBuf::from("config/agent-credential-audit"),
        );
        let intent = intent().unwrap();

        audit.record_intent(&intent).unwrap();
        assert!(audit.record_intent(&intent).is_err());

        let path = audit
            .trusted_root
            .join(&audit.relative_directory)
            .join(format!("{}-intent.json", intent.correlation().as_str()));
        let record = fs::read_to_string(path).unwrap();
        assert!(record.contains("so.nessa.agent-credentials"));
        assert!(record.contains("ci:one:claude-api-key"));
        assert!(record.contains("\"caller\":\"setup\""));
        assert!(!record.contains("private"));
    }

    #[cfg(unix)]
    #[test]
    fn an_intermediate_symlink_cannot_redirect_audit_records() {
        let root = tempfile::tempdir().unwrap();
        let trusted_root = root.path().join("private");
        let outside = root.path().join("outside");
        nessa_local_storage::create_directory(&trusted_root).unwrap();
        nessa_local_storage::create_directory(&outside).unwrap();
        symlink(&outside, trusted_root.join("config")).unwrap();
        let audit = FileCredentialSaveAudit::beneath(
            trusted_root,
            PathBuf::from("config/agent-credential-audit"),
        );

        assert!(audit.record_intent(&intent().unwrap()).is_err());
        assert!(std::fs::read_dir(outside).unwrap().next().is_none());
    }
}
