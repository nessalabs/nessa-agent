use crate::{
    application::{EndpointDiscovery, EndpointPublication, ManagedRuntimeAdvertisement},
    domain::GatewayEndpoint,
};
use serde::{Deserialize, Serialize};
use std::{
    io::{self, ErrorKind, Read, Write},
    net::{SocketAddr, TcpStream},
    path::PathBuf,
    time::Duration,
};

pub const ENDPOINT_FILE: &str = "gateway-endpoint.json";

/// Atomically replaces the endpoint record in this stage and instance's log directory.
pub struct FileEndpointPublication {
    directory: PathBuf,
}

impl FileEndpointPublication {
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EndpointRecord {
    web_socket_url: String,
    endpoint_instance: String,
    process_id: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime_fingerprint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    service_generation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime_instance: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime_process_id: Option<u32>,
}

impl EndpointPublication for FileEndpointPublication {
    fn publish(
        &self,
        endpoint: &GatewayEndpoint,
        managed: Option<&ManagedRuntimeAdvertisement>,
    ) -> io::Result<()> {
        nessa_local_storage::create_directory(&self.directory)?;
        let mut file = nessa_local_storage::PrivateTempFile::new_in(&self.directory)?;
        let record = EndpointRecord {
            web_socket_url: endpoint.web_socket_url(),
            endpoint_instance: endpoint.identity().instance().to_owned(),
            process_id: endpoint.identity().process_id(),
            runtime_fingerprint: managed.map(|value| value.fingerprint.clone()),
            service_generation: managed.map(|value| value.generation.clone()),
            runtime_instance: managed.map(|value| value.instance.clone()),
            runtime_process_id: managed.map(|value| value.process_id),
        };
        serde_json::to_writer(file.as_file_mut(), &record)?;
        file.as_file().sync_all()?;
        file.persist(&self.directory.join(ENDPOINT_FILE))?;
        nessa_local_storage::sync_directory(&self.directory)
    }
}

/// Private endpoint record plus bounded unauthenticated health correlation.
pub struct FileEndpointDiscovery {
    directory: PathBuf,
}

impl FileEndpointDiscovery {
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }
}

impl EndpointDiscovery for FileEndpointDiscovery {
    fn discover(&self) -> io::Result<Option<GatewayEndpoint>> {
        let path = self.directory.join(ENDPOINT_FILE);
        let mut file = match nessa_local_storage::open(
            &path,
            nessa_local_storage::OpenMode::ReadNonblocking,
        ) {
            Ok(file) => file,
            // Absence and an unreadable boundary retain today's configured
            // address. A file we did read but could not trust fails below.
            Err(_) => return Ok(None),
        };
        let mut bytes = Vec::new();
        if file.by_ref().take(16_385).read_to_end(&mut bytes).is_err() {
            return Ok(None);
        }
        if bytes.is_empty() || bytes.len() > 16_384 {
            return Err(invalid_record());
        }
        let record: EndpointRecord =
            serde_json::from_slice(&bytes).map_err(|_| invalid_record())?;
        if serde_json::to_vec(&record).map_err(|_| invalid_record())? != bytes {
            return Err(invalid_record());
        }
        let address = record_address(&record)?;
        let health = read_health(address)?;
        if !health.matches(&record) {
            return Err(io::Error::new(
                ErrorKind::PermissionDenied,
                "published gateway endpoint belongs to a different process",
            ));
        }
        let identity =
            crate::domain::EndpointIdentity::new(record.endpoint_instance, record.process_id)
                .map_err(|_| invalid_record())?;
        GatewayEndpoint::new(address, identity)
            .map(Some)
            .map_err(|_| invalid_record())
    }
}

fn invalid_record() -> io::Error {
    io::Error::new(
        ErrorKind::InvalidData,
        "published gateway endpoint is malformed",
    )
}

fn record_address(record: &EndpointRecord) -> io::Result<SocketAddr> {
    if uuid::Uuid::parse_str(&record.endpoint_instance)
        .ok()
        .is_none_or(|value| value.to_string() != record.endpoint_instance)
        || record.process_id == 0
    {
        return Err(invalid_record());
    }
    let managed = [
        record.runtime_fingerprint.is_some(),
        record.service_generation.is_some(),
        record.runtime_instance.is_some(),
        record.runtime_process_id.is_some(),
    ];
    if managed.iter().any(|value| *value) && !managed.iter().all(|value| *value) {
        return Err(invalid_record());
    }
    if let (Some(fingerprint), Some(generation), Some(instance), Some(process_id)) = (
        &record.runtime_fingerprint,
        &record.service_generation,
        &record.runtime_instance,
        record.runtime_process_id,
    ) {
        let digest = |value: &str| {
            value.len() == 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        };
        if !digest(fingerprint)
            || !digest(generation)
            || uuid::Uuid::parse_str(instance)
                .ok()
                .is_none_or(|value| value.to_string() != *instance)
            || instance != &record.endpoint_instance
            || process_id != record.process_id
        {
            return Err(invalid_record());
        }
    }
    let url = url::Url::parse(&record.web_socket_url).map_err(|_| invalid_record())?;
    if url.scheme() != "ws"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid_record());
    }
    let host = url.host_str().ok_or_else(invalid_record)?;
    let ip: std::net::IpAddr = host.parse().map_err(|_| invalid_record())?;
    let port = url.port().ok_or_else(invalid_record)?;
    if !ip.is_loopback() || port == 0 {
        return Err(invalid_record());
    }
    Ok(SocketAddr::new(ip, port))
}

struct HealthIdentity {
    endpoint_instance: String,
    endpoint_process_id: String,
    runtime_fingerprint: Option<String>,
    service_generation: Option<String>,
    runtime_instance: Option<String>,
    runtime_process_id: Option<String>,
}

impl HealthIdentity {
    fn matches(&self, record: &EndpointRecord) -> bool {
        let runtime_process_id = record.runtime_process_id.map(|value| value.to_string());
        self.endpoint_instance == record.endpoint_instance
            && self.endpoint_process_id == record.process_id.to_string()
            && self.runtime_fingerprint.as_ref() == record.runtime_fingerprint.as_ref()
            && self.service_generation.as_ref() == record.service_generation.as_ref()
            && self.runtime_instance.as_ref() == record.runtime_instance.as_ref()
            && self.runtime_process_id.as_ref() == runtime_process_id.as_ref()
    }
}

fn read_health(address: SocketAddr) -> io::Result<HealthIdentity> {
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_millis(300))?;
    stream.set_read_timeout(Some(Duration::from_millis(500)))?;
    stream.set_write_timeout(Some(Duration::from_millis(500)))?;
    stream.write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")?;
    let mut bytes = Vec::new();
    stream.take(8192).read_to_end(&mut bytes)?;
    parse_health(&bytes).ok_or_else(|| {
        io::Error::new(
            ErrorKind::PermissionDenied,
            "published gateway endpoint health is malformed",
        )
    })
}

fn parse_health(bytes: &[u8]) -> Option<HealthIdentity> {
    let text = std::str::from_utf8(bytes).ok()?;
    let (headers, _) = text.split_once("\r\n\r\n")?;
    let mut lines = headers.split("\r\n");
    if lines.next()?.split_whitespace().nth(1)? != "200" {
        return None;
    }
    let mut values = std::collections::HashMap::new();
    for line in lines {
        let (name, value) = line.split_once(':')?;
        let name = name.to_ascii_lowercase();
        if !matches!(
            name.as_str(),
            "x-nessa-endpoint-instance"
                | "x-nessa-endpoint-process-id"
                | "x-nessa-runtime-fingerprint"
                | "x-nessa-service-generation"
                | "x-nessa-runtime-instance"
                | "x-nessa-process-id"
        ) {
            continue;
        }
        if values.insert(name, value.trim().to_owned()).is_some() {
            return None;
        }
    }
    Some(HealthIdentity {
        endpoint_instance: values.remove("x-nessa-endpoint-instance")?,
        endpoint_process_id: values.remove("x-nessa-endpoint-process-id")?,
        runtime_fingerprint: values.remove("x-nessa-runtime-fingerprint"),
        service_generation: values.remove("x-nessa-service-generation"),
        runtime_instance: values.remove("x-nessa-runtime-instance"),
        runtime_process_id: values.remove("x-nessa-process-id"),
    })
}
