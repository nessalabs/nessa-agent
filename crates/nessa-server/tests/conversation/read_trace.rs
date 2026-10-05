//! A refused `conversation.read` is a warning on the gateway's tracing
//! subscriber, with the conversation and why. The ask is the debug event on
//! the same span. Neither carries the error's own text.
use super::{
    trace_conversation_index, trace_conversation_read, AgentError, ConversationError,
    ConversationId,
};
use std::sync::{Arc, Mutex, PoisonError};

#[test]
fn a_refused_conversation_read_is_traced_with_the_conversation_and_why() {
    let id = ConversationId::new("11111111-1111-4111-8111-111111111111").unwrap();
    let text = captured(|| {
        trace_conversation_read(&id, Some(&ConversationError::Unavailable));
        trace_conversation_read(&id, Some(&ConversationError::Agent(AgentError::Busy)));
        trace_conversation_read(
            &id,
            Some(&ConversationError::Agent(AgentError::Transport(
                "pipe closed".into(),
            ))),
        );
        trace_conversation_index(Some(&ConversationError::Unavailable));
    });
    assert!(text.contains("conversation read asked"), "{text}");
    assert!(text.contains("conversation read refused"), "{text}");
    assert!(
        text.contains("11111111-1111-4111-8111-111111111111"),
        "{text}"
    );
    assert!(text.contains("method=\"conversation.read\""), "{text}");
    assert!(text.contains("subject=\"conversation\""), "{text}");
    assert!(text.contains("code=\"temporarily_unavailable\""), "{text}");
    assert!(text.contains("hint=\"unavailable\""), "{text}");
    // Busy and a socket share `agent_operation_failed` on the wire. The hint
    // is what tells them apart, and it is not the transport's own text.
    assert!(text.contains("code=\"agent_operation_failed\""), "{text}");
    assert!(text.contains("hint=\"busy\""), "{text}");
    assert!(text.contains("hint=\"socket\""), "{text}");
    assert!(!text.contains("pipe closed"), "{text}");
    // The index is the list, not a conversation.
    assert!(text.contains("conversation index asked"), "{text}");
    assert!(text.contains("conversation index refused"), "{text}");
    assert!(text.contains("method=\"conversation.list\""), "{text}");
    assert!(text.contains("subject=\"index\""), "{text}");
}

#[test]
fn a_conversation_read_that_answers_is_not_a_refusal() {
    let id = ConversationId::new("11111111-1111-4111-8111-111111111111").unwrap();
    let text = captured(|| trace_conversation_read(&id, None));
    assert!(text.contains("conversation read asked"), "{text}");
    assert!(!text.contains("conversation read refused"), "{text}");
    assert!(!text.contains("code="), "{text}");
}

fn captured(log: impl FnOnce()) -> String {
    let captured = Capture(Arc::new(Mutex::new(Vec::new())));
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_writer(captured.clone())
        .with_ansi(false)
        .finish();
    tracing::subscriber::with_default(subscriber, log);
    let bytes = captured
        .0
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    String::from_utf8(bytes).unwrap()
}

#[derive(Clone)]
struct Capture(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for Capture {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(buffer);
        Ok(buffer.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Capture {
    type Writer = Self;
    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}
