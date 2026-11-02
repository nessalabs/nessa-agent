//! Voice chat: the realtime voice key the gateway keeps, and the short-lived
//! calls it mints for a window to talk to a conversation through.
//!
//! ```text
//! voice.status/saveKey/forgetKey ──▶ VoiceCalls ──▶ SecretStore (vault entry)
//!                                          └──▶ VoiceAudit (requested, then failure; no key)
//! voice.startCall ──▶ VoiceCalls ──▶ RealtimeProvider (OpenAI, or a scripted fake)
//! ```
//!
//! Arrows are calls. The window holds the call; the gateway holds the key
//! and fixes what every call is told and may call when it mints it. The
//! states are in `docs/design/voice-chat.md`.
pub mod application;
pub mod domain;
pub mod infrastructure;

#[cfg(test)]
#[path = "../../tests/voice/mod.rs"]
mod tests;
