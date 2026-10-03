//! One first-publication intent and outcome, correlated across exact restoration.
use super::*;
use crate::adapters::pairing::NativeIdentity;
use serde::{Deserialize, Serialize};

pub(super) const INTENT_FILE: &str = "gateway-key-intent.json";
pub(super) const OUTCOME_FILE: &str = "gateway-key-outcome.json";
const AUDIT_BYTES: usize = 2048;
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct KeyIntent {
    operation: [u8; 16],
    gateway: String,
    before: Before,
    cause: Cause,
    initiator: Actor,
    observed_at_ms: u64,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct KeyOutcome {
    intent: KeyIntent,
    after: After,
    public_key: [u8; 32],
    acknowledged_at_ms: u64,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Before {
    Absent,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum After {
    Published,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Cause {
    FirstPublication,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Actor {
    System,
}

impl FilePairingState {
    fn key_bytes(&self) -> Result<Option<Zeroizing<Vec<u8>>>, PrivateStateError> {
        let bytes = self.read(GATEWAY_FILE, KEY_BYTES)?;
        if bytes.as_ref().is_some_and(|bytes| &bytes[..4] != KEY_MAGIC) {
            return Err(PrivateStateError::Corrupt);
        }
        Ok(bytes)
    }
    fn key_intent(&self, gateway: &AudienceId) -> Result<Option<KeyIntent>, PrivateStateError> {
        self.read_bounded(INTENT_FILE, AUDIT_BYTES)?
            .map(|bytes| {
                let intent: KeyIntent =
                    serde_json::from_slice(&bytes).map_err(|_| PrivateStateError::Corrupt)?;
                let target = AudienceId::new(intent.gateway.clone())
                    .map_err(|_| PrivateStateError::Corrupt)?;
                if &target != gateway {
                    return Err(PrivateStateError::Conflict);
                }
                Ok(intent)
            })
            .transpose()
    }
    fn key_outcome(&self) -> Result<Option<KeyOutcome>, PrivateStateError> {
        self.read_bounded(OUTCOME_FILE, AUDIT_BYTES)?
            .map(|bytes| serde_json::from_slice(&bytes).map_err(|_| PrivateStateError::Corrupt))
            .transpose()
    }
    fn validate_key_outcome_presence(&self, key_present: bool) -> Result<(), PrivateStateError> {
        if self.key_outcome()?.is_some() && !key_present {
            return Err(PrivateStateError::Corrupt);
        }
        Ok(())
    }
    fn acknowledge_key(
        &self,
        key: &PrivateKeyMaterial,
        intent: &KeyIntent,
        clock: &dyn Clock,
    ) -> Result<(), PrivateStateError> {
        // Derivation asks the native key owner. The additional Nessa-owned seed
        // copy remains zeroizing; ring's internal erasure is not claimed.
        let identity =
            NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new(*key.expose_bytes())))
                .map_err(|_| PrivateStateError::Unavailable)?;
        let public_key = identity.public_spki()[12..]
            .try_into()
            .map_err(|_| PrivateStateError::Corrupt)?;
        if let Some(saved) = self.key_outcome()? {
            if saved.intent != *intent || saved.public_key != public_key {
                return Err(PrivateStateError::Conflict);
            }
            let bytes = audit_bytes(&saved)?;
            return self
                .reconcile(OUTCOME_FILE, &bytes)
                .map_err(|error| audit_error(error, GatewayPublicationState::Published));
        }
        let outcome = KeyOutcome {
            intent: intent.clone(),
            after: After::Published,
            public_key,
            acknowledged_at_ms: clock.unix_milliseconds(),
        };
        self.publish(OUTCOME_FILE, &audit_bytes(&outcome)?, false)
            .map_err(|error| audit_error(error, GatewayPublicationState::Published))
    }
}
impl GatewayKeyStore for FilePairingState {
    fn restore_gateway_key(
        &self,
        gateway: &AudienceId,
        clock: &dyn Clock,
    ) -> Result<Option<PrivateKeyMaterial>, PrivateStateError> {
        let _guard = self
            .operation
            .lock()
            .map_err(|_| PrivateStateError::Unavailable)?;
        let bytes = self.key_bytes()?;
        let intent = self.key_intent(gateway)?;
        let Some(bytes) = bytes else {
            self.validate_key_outcome_presence(false)?;
            // An unfinished original intent may continue only under this lifetime
            // lock, after coherent absence of any original key-file effect.
            return Ok(None);
        };
        let intent = intent.ok_or(PrivateStateError::Corrupt)?;
        self.reconcile(GATEWAY_FILE, &bytes)?;
        let mut seed = Zeroizing::new([0; 32]);
        seed.copy_from_slice(&bytes[4..]);
        let key = PrivateKeyMaterial::new(seed);
        self.acknowledge_key(&key, &intent, clock)?;
        Ok(Some(key))
    }
    fn save_gateway_key(
        &self,
        key: &PrivateKeyMaterial,
        gateway: &AudienceId,
        clock: &dyn Clock,
    ) -> Result<(), PrivateStateError> {
        let _guard = self
            .operation
            .lock()
            .map_err(|_| PrivateStateError::Unavailable)?;
        let mut bytes = Zeroizing::new(Vec::with_capacity(KEY_BYTES));
        bytes.extend_from_slice(KEY_MAGIC);
        bytes.extend_from_slice(key.expose_bytes());
        let saved = self.key_bytes()?;
        if saved
            .as_ref()
            .is_some_and(|saved| !bool::from(saved.as_slice().ct_eq(bytes.as_slice())))
        {
            return Err(PrivateStateError::Conflict);
        }
        let publication = if saved.is_some() {
            GatewayPublicationState::Published
        } else {
            GatewayPublicationState::NotPublished
        };
        let intent = match self.key_intent(gateway)? {
            Some(intent) => intent,
            None if saved.is_some() => return Err(PrivateStateError::Corrupt),
            None => {
                self.validate_key_outcome_presence(false)?;
                let mut operation = [0; 16];
                getrandom::fill(&mut operation).map_err(|_| PrivateStateError::Unavailable)?;
                let intent = KeyIntent {
                    operation,
                    gateway: gateway.as_str().to_owned(),
                    before: Before::Absent,
                    cause: Cause::FirstPublication,
                    initiator: Actor::System,
                    observed_at_ms: clock.unix_milliseconds(),
                };
                self.publish(INTENT_FILE, &audit_bytes(&intent)?, false)
                    .map_err(|error| audit_error(error, publication))?;
                intent
            }
        };
        // Validate an existing outcome before any key-file effect.
        self.validate_key_outcome_presence(saved.is_some())?;
        if saved.is_some() {
            self.reconcile(GATEWAY_FILE, &bytes)?;
        } else {
            self.publish(GATEWAY_FILE, &bytes, false)?;
        }
        self.acknowledge_key(key, &intent, clock)
    }
}
fn audit_bytes(value: &impl Serialize) -> Result<Vec<u8>, PrivateStateError> {
    let bytes = serde_json::to_vec(value).map_err(|_| PrivateStateError::Corrupt)?;
    if bytes.len() > AUDIT_BYTES {
        return Err(PrivateStateError::Corrupt);
    }
    Ok(bytes)
}
pub(super) fn audit_error(
    error: PrivateStateError,
    publication: GatewayPublicationState,
) -> PrivateStateError {
    match error {
        PrivateStateError::Conflict | PrivateStateError::Corrupt => error,
        PrivateStateError::Publication(failure) => PrivateStateError::AuditPublication {
            key: publication,
            failure,
        },
        PrivateStateError::AuditPublication { .. } => error,
        _ => PrivateStateError::AuditUnavailable(publication),
    }
}
