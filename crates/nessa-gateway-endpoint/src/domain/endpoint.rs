use std::net::SocketAddr;
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
    address: SocketAddr,
    identity: EndpointIdentity,
}

impl GatewayEndpoint {
    pub fn new(address: SocketAddr, identity: EndpointIdentity) -> Result<Self, &'static str> {
        if !address.ip().is_loopback() || address.port() == 0 {
            return Err("gateway endpoint must be a bound loopback socket");
        }
        Ok(Self { address, identity })
    }

    pub fn web_socket_url(&self) -> String {
        format!("ws://{}", self.address)
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn identity(&self) -> &EndpointIdentity {
        &self.identity
    }
}
