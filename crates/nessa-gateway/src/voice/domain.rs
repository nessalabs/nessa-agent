//! What a voice call is: the saved key's shape, what every call is told and
//! may call, and what the window is given to connect one. No I/O here.
use zeroize::Zeroizing;

/// The longest key the wire admits (`VoiceSaveKeyParams.key`).
pub const MAX_KEY_BYTES: usize = 256;

/// A provider API key as the gateway holds it: printable ASCII without
/// spaces, 1 to [`MAX_KEY_BYTES`] bytes, zeroed when dropped. The schema
/// says the same at the wire; this type is what the vault entry is read
/// back into, so a corrupted entry is refused here rather than sent.
pub struct VoiceKey(Zeroizing<String>);

impl VoiceKey {
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let shaped = !bytes.is_empty()
            && bytes.len() <= MAX_KEY_BYTES
            && bytes.iter().all(|byte| (b'!'..=b'~').contains(byte));
        shaped.then(|| {
            Self(Zeroizing::new(
                std::str::from_utf8(bytes)
                    .expect("printable ASCII is UTF-8")
                    .to_owned(),
            ))
        })
    }

    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl std::fmt::Debug for VoiceKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("VoiceKey(…)")
    }
}

/// The realtime model every call runs.
pub const MODEL: &str = "gpt-realtime";
/// The voice it speaks in.
pub const VOICE: &str = "marin";
/// The model that writes down what the person said, which is what the agent
/// is sent.
pub const TRANSCRIPTION_MODEL: &str = "gpt-4o-transcribe";
/// How long a minted secret can open a call. The window connects at once.
pub const SECRET_SECONDS: u64 = 60;

/// What every call is told. The coordinator rule is the product's: the
/// voice hands work to the conversation's agent and talks about its
/// answers, and never answers a request itself.
pub const INSTRUCTIONS: &str = "You are Nessa's voice in one chat. You coordinate; you never do the work yourself. A coding agent in this chat does the work.\n\
\n\
- When the person asks for anything to be done, checked, looked up, explained from their project, written or changed, call send_to_agent at once. You pass no text: the person's own words are sent. Then say one very short acknowledgement, such as \"On it.\"\n\
- Never answer such a request yourself, never guess, and never make up what the agent found or did.\n\
- If send_to_agent answers \"unknown\", say you are not sure it went through and that they can check the chat; do not send it again unless they ask.\n\
- When the person asks to stop or cancel the agent, call stop_agent.\n\
- A message that starts with \"Agent reply:\" is the agent's answer. Tell the person what it says in a few natural spoken sentences. Leave out code, tables, file paths and long lists, and say the details are on screen.\n\
- A message that starts with \"Chat so far:\" is the chat before the call, for context only. Do not read it out.\n\
- You may answer briefly about the call itself, or ask the person to say more when their request is unclear.\n\
- Speak briefly and warmly, in the person's language.";

/// What the transcription model is asked for: the person's words, tidied.
pub const TRANSCRIPTION_PROMPT: &str =
    "Clean, punctuated sentences. Leave out filler words such as um and uh, and false starts.";

/// The voice's two tools, by the names the window answers to
/// (`VoiceToolName` on the wire), each with what it tells the model.
pub const TOOLS: [(&str, &str); 2] = [
    (
        "send_to_agent",
        "Send what the person just asked to the chat's agent, in their own words.",
    ),
    ("stop_agent", "Stop the agent's running turn."),
];

/// How the window connects a call (`VoiceTransport` on the wire).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallTransport {
    /// OpenAI's realtime API over WebRTC.
    WebRtc,
    /// A provider's realtime events over a WebSocket, with no audio.
    Scripted,
}

/// A call the window may connect: where, how, and the short-lived secret
/// that opens it. Never the key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallGrant {
    pub transport: CallTransport,
    pub call_url: String,
    pub client_secret: String,
    pub expires_at_ms: u64,
    pub model: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_printable_ascii_without_spaces_and_bounded() {
        assert!(VoiceKey::parse(b"sk-proj-abc_123").is_some());
        assert!(VoiceKey::parse(b"").is_none());
        assert!(VoiceKey::parse(b"sk abc").is_none());
        assert!(VoiceKey::parse("sk-é".as_bytes()).is_none());
        assert!(VoiceKey::parse(&[b'a'; MAX_KEY_BYTES]).is_some());
        assert!(VoiceKey::parse(&[b'a'; MAX_KEY_BYTES + 1]).is_none());
    }

    #[test]
    fn a_key_never_prints() {
        let key = VoiceKey::parse(b"sk-secret").unwrap();
        assert_eq!(format!("{key:?}"), "VoiceKey(…)");
    }
}
