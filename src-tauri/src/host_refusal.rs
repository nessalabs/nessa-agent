//! Why a window was not given the local server, as a value the page can branch
//! on. The sentences live in `src/host/startup-refusals.json`, which the page
//! reads too: this module formats them for the host's log and for the document
//! it shows when the dev server never answers.

use crate::gateway::infrastructure::GatewayUnread;
use crate::gateway_endpoint::application::EndpointError;
use serde::Serialize;
use serde_json::Value;

fn copies() -> Value {
    serde_json::from_str(include_str!("../../src/host/startup-refusals.json"))
        .expect("startup refusal sentences are JSON")
}

pub(crate) fn sentence(key: &str) -> String {
    copies()
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or(key)
        .to_string()
}

pub(crate) fn fill(key: &str, values: &[(&str, &str)]) -> String {
    let mut text = sentence(key);
    for (name, value) in values {
        text = text.replace(&format!("{{{name}}}"), value);
    }
    text
}

/// The sentence a person reads. The log sentences stay on [`sentence`].
pub(crate) fn line() -> String {
    sentence("line")
}

/// The copyable token for `key` (`STARTUP_GATEWAY` and the rest).
pub(crate) fn code(key: &str) -> String {
    copies()
        .get("code")
        .and_then(|table| table.get(key))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(key)
        .to_string()
}

/// A refusal the page branches on. Other failures stay a sentence
/// ([`SurfaceCommandError::Message`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostRefusal {
    pub reason: HostRefusalReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum HostRefusalReason {
    NotProvisioned,
    NotReady,
    WrongStage,
}

/// What a gateway command returns to the page: a typed refusal, or the
/// sentence an untyped failure already had.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum SurfaceCommandError {
    Refusal(HostRefusal),
    Message(String),
}

impl SurfaceCommandError {
    pub(crate) fn not_provisioned() -> Self {
        Self::Refusal(HostRefusal {
            reason: HostRefusalReason::NotProvisioned,
            bundle: None,
            requested: None,
        })
    }

    pub(crate) fn not_ready() -> Self {
        Self::Refusal(HostRefusal {
            reason: HostRefusalReason::NotReady,
            bundle: None,
            requested: None,
        })
    }

    pub(crate) fn wrong_stage(bundle: impl Into<String>, requested: impl Into<String>) -> Self {
        let bundle = bundle.into();
        let requested = requested.into();
        Self::Refusal(HostRefusal {
            reason: HostRefusalReason::WrongStage,
            bundle: Some(bundle),
            requested: Some(requested),
        })
    }

    pub(crate) fn message(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }
}

impl From<GatewayUnread> for SurfaceCommandError {
    fn from(error: GatewayUnread) -> Self {
        match error {
            GatewayUnread::NotReady => Self::not_ready(),
            GatewayUnread::Other(message) => Self::message(message),
        }
    }
}

impl From<EndpointError> for SurfaceCommandError {
    fn from(error: EndpointError) -> Self {
        match error {
            EndpointError::WrongStage { bundle, requested } => Self::wrong_stage(bundle, requested),
            other => Self::message(other.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_typed_refusal_serializes_as_its_reason_and_a_sentence_stays_a_string() {
        let typed = serde_json::to_value(SurfaceCommandError::wrong_stage("dev", "prod")).unwrap();
        assert_eq!(typed["reason"], "wrong-stage");
        assert_eq!(typed["bundle"], "dev");
        assert_eq!(typed["requested"], "prod");
        assert!(typed.get("message").is_none());

        let sentence = serde_json::to_value(SurfaceCommandError::message("disk fell off")).unwrap();
        assert_eq!(sentence, "disk fell off");
    }

    #[test]
    fn the_sentences_name_the_repair_and_both_stages() {
        let absent = sentence("not-provisioned");
        assert!(absent.contains("just start"), "{absent}");
        assert!(absent.contains("without the local server"), "{absent}");
        let mismatch = fill("wrong-stage", &[("bundle", "dev"), ("requested", "prod")]);
        assert!(
            mismatch.contains("dev") && mismatch.contains("prod"),
            "{mismatch}"
        );
        assert!(!mismatch.contains("{bundle}"), "{mismatch}");
    }

    #[test]
    fn the_screen_reads_a_line_and_a_code_and_the_log_stays_a_sentence() {
        let said = line();
        assert!(said.contains("couldn’t start"), "{said}");
        assert!(!said.contains("just start"), "{said}");
        assert_eq!(code("document-unserved"), "STARTUP_PAGE");
        assert_eq!(code("not-listening"), "STARTUP_GATEWAY");
        assert_eq!(code("wrong-stage"), "STARTUP_STAGE");
        assert_eq!(code("host"), "STARTUP_HOST");
        let absent = sentence("not-provisioned");
        assert!(absent.contains("just start"), "{absent}");
    }

    #[test]
    fn an_unread_gateway_and_an_endpoint_error_share_one_surface_error() {
        assert_eq!(
            SurfaceCommandError::from(GatewayUnread::NotReady),
            SurfaceCommandError::not_ready()
        );
        assert_eq!(
            SurfaceCommandError::from(GatewayUnread::Other("disk".into())),
            SurfaceCommandError::message("disk")
        );
        assert_eq!(
            SurfaceCommandError::from(EndpointError::WrongStage {
                bundle: "dev".into(),
                requested: "prod".into(),
            }),
            SurfaceCommandError::wrong_stage("dev", "prod")
        );
        assert_eq!(
            SurfaceCommandError::from(EndpointError::DestinationMismatch),
            SurfaceCommandError::message(
                "The credential request does not match the verified gateway endpoint"
            )
        );
    }
}
