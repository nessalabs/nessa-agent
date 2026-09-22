use uuid::Uuid;

/// Identity of one server process publishing and answering for an endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EndpointIdentity {
    instance: String,
    process_id: u32,
}

impl EndpointIdentity {
    pub fn new(instance: String, process_id: u32) -> Result<Self, &'static str> {
        let parsed = Uuid::parse_str(&instance).map_err(|_| "invalid endpoint instance")?;
        if parsed.to_string() != instance {
            return Err("noncanonical endpoint instance");
        }
        if process_id == 0 {
            return Err("invalid endpoint process ID");
        }
        Ok(Self {
            instance,
            process_id,
        })
    }

    pub fn instance(&self) -> &str {
        &self.instance
    }

    pub fn process_id(&self) -> u32 {
        self.process_id
    }
}

/// One numeric-loopback WebSocket endpoint and the process that owns it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GatewayEndpoint {
    web_socket_url: String,
    identity: EndpointIdentity,
}

impl GatewayEndpoint {
    pub fn new(web_socket_url: String, identity: EndpointIdentity) -> Result<Self, &'static str> {
        let parsed = url::Url::parse(&web_socket_url).map_err(|_| "invalid gateway endpoint")?;
        let loopback = match parsed.host() {
            Some(url::Host::Ipv4(address)) => address == std::net::Ipv4Addr::LOCALHOST,
            Some(url::Host::Ipv6(address)) => address == std::net::Ipv6Addr::LOCALHOST,
            _ => false,
        };
        let explicit_port = web_socket_url
            .rsplit_once(':')
            .and_then(|(_, value)| value.parse::<u16>().ok());
        if parsed.scheme() != "ws"
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || explicit_port.is_none_or(|port| port == 0)
            || parsed.path() != "/"
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || web_socket_url.ends_with('/')
            || !loopback
        {
            return Err("gateway endpoint must be a bound loopback socket");
        }
        Ok(Self {
            web_socket_url,
            identity,
        })
    }

    pub fn web_socket_url(&self) -> String {
        self.web_socket_url.clone()
    }

    pub fn identity(&self) -> &EndpointIdentity {
        &self.identity
    }
}

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
