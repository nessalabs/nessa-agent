//! Voice chat, composed once per run: the key's encrypted vault entry, the
//! realtime provider calls are minted from, and the audit of both.
//!
//! `NESSA_VOICE_REALTIME` names a file describing another provider in place
//! of OpenAI, so the stress check can point a gateway at a fake on this
//! machine. What such a file may say is [`RealtimeEndpoint::parse`]'s; this
//! only reads it.
use std::{path::Path, sync::Arc};

use super::local_auth::{conversation_root, SystemClock};
use crate::voice::application::VoiceCalls;
use crate::voice::infrastructure::{DurableVoiceAudit, RealtimeEndpoint, RealtimeHttps};

/// The voice calls kept under `namespace`, or `None` — logged — when the
/// provider file cannot be used or no HTTP client can be built. Without them
/// the voice methods answer `voice_not_configured`.
pub(super) fn compose(
    namespace: &Path,
    vault: Arc<nessa_vault::application::Vault>,
) -> Option<Arc<VoiceCalls>> {
    let endpoint = match std::env::var_os("NESSA_VOICE_REALTIME") {
        None => RealtimeEndpoint::openai(),
        Some(path) => match std::fs::read(&path).map(|bytes| RealtimeEndpoint::parse(&bytes)) {
            Ok(Ok(endpoint)) => endpoint,
            Ok(Err(problem)) => {
                tracing::error!(
                    ?problem,
                    "voice is off this run: NESSA_VOICE_REALTIME does not describe a provider"
                );
                return None;
            }
            Err(error) => {
                tracing::error!(%error, "voice is off this run: NESSA_VOICE_REALTIME could not be read");
                return None;
            }
        },
    };
    let Some(provider) = RealtimeHttps::new(endpoint) else {
        tracing::error!("voice is off this run: no HTTPS client could be built");
        return None;
    };
    let key = crate::secrets::VaultSecret::new(
        vault,
        nessa_vault::domain::EntryName::new("voice-openai-key").expect("static vault entry"),
    );
    Some(Arc::new(VoiceCalls::new(
        Arc::new(key),
        Arc::new(provider),
        Arc::new(DurableVoiceAudit::new(
            conversation_root(namespace).join("audit").join("voice"),
            Arc::new(SystemClock),
        )),
    )))
}
