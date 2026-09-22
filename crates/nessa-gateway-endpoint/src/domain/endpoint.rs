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
