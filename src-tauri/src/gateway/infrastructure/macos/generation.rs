//! Persistent installation identity. Reuse the published generation only for the
//! same complete definition while it is unfenced; replacement always gets fresh entropy.
use super::control::RetirementEvidence;
use serde_json::Value;
use std::{fs::File, io::Read};

pub(super) fn service_generation(
    desired: &Value,
    installed: Option<&Value>,
    fence: Option<&RetirementEvidence>,
) -> Result<String, String> {
    select_generation(desired, installed, fence, random_generation)
}
pub(super) fn random_generation() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(|error| error.to_string())?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}
fn select_generation(
    desired: &Value,
    installed: Option<&Value>,
    fence: Option<&RetirementEvidence>,
    fresh: impl FnOnce() -> Result<String, String>,
) -> Result<String, String> {
    if desired
        .get("EnvironmentVariables")
        .and_then(|environment| environment.get("NESSA_SERVICE_GENERATION"))
        .is_some()
    {
        return Err("Desired base definition already contains a service generation".into());
    }
    if let Some(installed) = installed {
        let mut base = installed.clone();
        let generation = base
            .get_mut("EnvironmentVariables")
            .and_then(Value::as_object_mut)
            .and_then(|environment| environment.remove("NESSA_SERVICE_GENERATION"));
        if let Some(Value::String(generation)) = generation {
            let fingerprint = base
                .get("EnvironmentVariables")
                .and_then(|environment| environment.get("NESSA_RUNTIME_FINGERPRINT"))
                .and_then(Value::as_str);
            let fenced = fingerprint.is_some_and(|fingerprint| {
                fence.is_some_and(|fence| fence.matches(fingerprint, &generation))
            });
            if &base == desired
                && !fenced
                && generation.len() == 64
                && generation
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
            {
                return Ok(generation);
            }
        }
    }
    fresh()
}
#[cfg(test)]
#[path = "../../../../tests/gateway/infrastructure/generation.rs"]
mod tests;
