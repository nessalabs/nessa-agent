//! Current purpose refusal observations retain canonical evidence without body data.
use super::*;
use crate::conversation::application::{AdmitOwnerPhoneReader, OwnerPhoneReader};
use nessa_protocol::conversation::read_scope::ReadRefusal;
use std::{
    io::{self, Write},
    sync::Mutex,
};
use tracing::instrument::WithSubscriber;

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);
struct Writer(Arc<Mutex<Vec<u8>>>);
impl Write for Writer {
    fn write(&mut self, value: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(value);
        Ok(value.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Capture {
    type Writer = Writer;
    fn make_writer(&'a self) -> Writer {
        Writer(self.0.clone())
    }
}
fn subscriber(output: Capture) -> impl tracing::Subscriber + Send + Sync {
    tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_max_level(tracing::Level::DEBUG)
        .with_writer(output)
        .finish()
}
fn text(output: &Capture) -> String {
    String::from_utf8(output.0.lock().unwrap().clone()).unwrap()
}
fn safe(text: &str, f: &Fixture, credential: &CredentialId) {
    assert!(text.contains("gateway=gateway"));
    assert!(text.contains("organization=org"));
    assert!(text.contains(&format!("credential={}", credential.as_str())));
    assert!(text.contains(&format!("receiver={}", f.receiver.receiver_id)));
    for forbidden in [
        "bearer",
        "signature",
        "private_key",
        "signer",
        "digest",
        "setup_code",
        "request_body",
        "prompt",
        "secret-sentinel",
    ] {
        assert!(!text.contains(forbidden), "unsafe trace field: {forbidden}");
    }
}
async fn reader(f: &Fixture, policy: Arc<dyn OwnerPhonePolicyEvaluator>) -> OwnerPhoneReader {
    let holding = ActualHolding(f.receivers.clone());
    AdmitOwnerPhoneReader {
        purposes: f.registry.as_ref(),
        holding: Some(&holding),
        clock: &Time,
        gateway: &f.gateway,
        policy,
        access_policy: Some(crate::conversation_test_support::access_policy()),
    }
    .execute(&f.session)
    .await
    .unwrap()
    .unwrap()
}
#[tokio::test]
async fn owner_phone_refusal_trace_retains_actual_cedar_linkage_forbid() {
    let f = fixture().await;
    let holding = ActualHolding(f.receivers.clone());
    let purposes = FixedPurpose {
        registry: f.registry.as_ref(),
        session: &f.session,
    };
    let output = Capture::default();
    let result = AdmitOwnerPhoneReader {
        purposes: &purposes,
        holding: Some(&holding),
        clock: &Time,
        gateway: &f.gateway,
        policy: Arc::new(CedarOwnerPhonePolicyEvaluator::new().unwrap()),
        access_policy: Some(crate::conversation_test_support::access_policy()),
    }
    .execute(&f.bootstrap)
    .with_subscriber(subscriber(output.clone()))
    .await;
    assert!(matches!(result, Err(ReadRefusal::WrongOwner)));
    let trace = text(&output);
    safe(&trace, &f, f.bootstrap.context().credential_id());
    assert!(trace.contains("operation=\"phone.context\""));
    assert!(trace.contains("rules=[\"owner-phone.invalid-linkage\"]"));
    assert!(trace.contains("outcome=\"policy_deny\""));
}
#[tokio::test]
async fn owner_phone_refusal_trace_labels_actual_empty_default_denial() {
    let f = fixture().await;
    let r = reader(&f, Arc::new(CedarOwnerPhonePolicyEvaluator::new().unwrap())).await;
    let output = Capture::default();
    let result = tracing::subscriber::with_default(subscriber(output.clone()), || {
        r.admit_operation(OwnerPhoneOperation::ProductMethod {
            method: "future.unknown",
            published_action: None,
        })
    });
    assert_eq!(result, Err(ReadRefusal::WrongOwner));
    let trace = text(&output);
    safe(&trace, &f, f.session.context().credential_id());
    assert!(trace.contains("operation=\"product.request\""));
    assert!(trace.contains("method=Some(\"future.unknown\")"));
    assert!(trace.contains("method_truncated=false"));
    assert!(trace.contains("rules=[]"));
    assert!(trace.contains("outcome=\"default_deny\""));
}
struct FailedMethod(CedarOwnerPhonePolicyEvaluator);
impl OwnerPhonePolicyEvaluator for FailedMethod {
    fn evaluate(
        &self,
        facts: OwnerPhonePolicyFacts<'_>,
    ) -> Result<OwnerPhoneDecision, AccessError> {
        if matches!(facts.operation, OwnerPhoneOperation::ProductMethod { .. }) {
            Err(AccessError::Unavailable)
        } else {
            self.0.evaluate(facts)
        }
    }
}
#[tokio::test]
async fn owner_phone_refusal_trace_preserves_typed_evaluator_failure() {
    let f = fixture().await;
    let r = reader(
        &f,
        Arc::new(FailedMethod(CedarOwnerPhonePolicyEvaluator::new().unwrap())),
    )
    .await;
    let output = Capture::default();
    let result = tracing::subscriber::with_default(subscriber(output.clone()), || {
        r.admit_operation(OwnerPhoneOperation::ProductMethod {
            method: "server.health",
            published_action: Some("server.read"),
        })
    });
    assert_eq!(result, Err(ReadRefusal::Unverifiable));
    let trace = text(&output);
    safe(&trace, &f, f.session.context().credential_id());
    assert!(trace.contains("method=Some(\"server.health\")"));
    assert!(trace.contains("error=Unavailable"));
    assert!(trace.contains("outcome=\"policy_error\""));
    assert!(!trace.contains("rules="));
}

#[tokio::test]
async fn owner_phone_method_trace_is_utf8_bounded_and_labels_truncation() {
    let f = fixture().await;
    let r = reader(&f, Arc::new(CedarOwnerPhonePolicyEvaluator::new().unwrap())).await;
    let method = format!("{}TAIL_NOT_TO_LOG", "é".repeat(129));
    let output = Capture::default();
    let result = tracing::subscriber::with_default(subscriber(output.clone()), || {
        r.admit_operation(OwnerPhoneOperation::ProductMethod {
            method: &method,
            published_action: None,
        })
    });
    assert_eq!(result, Err(ReadRefusal::WrongOwner));
    let trace = text(&output);
    assert!(trace.contains(&format!("method=Some({:?})", "é".repeat(128))));
    assert!(trace.contains("method_truncated=true"));
    assert!(trace.contains(&format!("method_bytes=Some({})", method.len())));
    assert!(!trace.contains("TAIL_NOT_TO_LOG"));
}
