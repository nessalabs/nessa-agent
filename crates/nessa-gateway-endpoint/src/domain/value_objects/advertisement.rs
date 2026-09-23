use super::{EndpointIdentity, GatewayEndpoint};

/// Identity fields supplied only by a desktop-managed gateway runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagedRuntimeIdentity {
    fingerprint: String,
    generation: String,
    endpoint: EndpointIdentity,
}

impl ManagedRuntimeIdentity {
    pub fn new(
        fingerprint: String,
        generation: String,
        instance: String,
        process_id: u32,
    ) -> Result<Self, &'static str> {
        let digest = |value: &str| {
            value.len() == 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        };
        if !digest(&fingerprint) || !digest(&generation) {
            return Err("invalid managed runtime digest");
        }
        Ok(Self {
            fingerprint,
            generation,
            endpoint: EndpointIdentity::new(instance, process_id)?,
        })
    }

    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub fn generation(&self) -> &str {
        &self.generation
    }

    pub fn endpoint(&self) -> &EndpointIdentity {
        &self.endpoint
    }
}

/// One internally consistent endpoint publication, with optional managed identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GatewayEndpointAdvertisement {
    endpoint: GatewayEndpoint,
    managed: Option<ManagedRuntimeIdentity>,
}

impl GatewayEndpointAdvertisement {
    pub fn new(
        endpoint: GatewayEndpoint,
        managed: Option<ManagedRuntimeIdentity>,
    ) -> Result<Self, &'static str> {
        if managed
            .as_ref()
            .is_some_and(|identity| identity.endpoint() != endpoint.identity())
        {
            return Err("managed runtime and endpoint identity disagree");
        }
        Ok(Self { endpoint, managed })
    }

    pub fn endpoint(&self) -> &GatewayEndpoint {
        &self.endpoint
    }

    pub fn managed(&self) -> Option<&ManagedRuntimeIdentity> {
        self.managed.as_ref()
    }
}
