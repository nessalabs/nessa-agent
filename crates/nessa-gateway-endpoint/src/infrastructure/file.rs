use crate::{
    application::{EndpointDiscovery, EndpointPublication},
    domain::{
        EndpointIdentity, GatewayEndpoint, GatewayEndpointAdvertisement, ManagedRuntimeIdentity,
    },
};
use serde::{Deserialize, Serialize};
use std::{
    io::{self, ErrorKind, Read, Write},
    net::{IpAddr, SocketAddr, TcpStream},
    path::PathBuf,
    time::{Duration, Instant},
};
use url::{Host, Url};

pub const ENDPOINT_FILE: &str = "gateway-endpoint.json";

/// Atomically replaces the endpoint record in this stage and instance's log directory.
///
/// `root` must already be a private directory owned by the current OS user;
/// `directory` is a relative namespace resolved by the caller.
pub struct FileEndpointPublication {
    root: PathBuf,
    directory: PathBuf,
}

impl FileEndpointPublication {
    pub fn new(root: PathBuf, directory: PathBuf) -> Self {
        Self { root, directory }
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
    fn publish(&self, advertisement: &GatewayEndpointAdvertisement) -> io::Result<()> {
        let endpoint = advertisement.endpoint();
        let managed = advertisement.managed();
        nessa_local_storage::create_directory_beneath(&self.root, &self.directory)
            .map_err(|error| publication_failed("create endpoint directory", error))?;
        let mut file =
            nessa_local_storage::PrivateTempFile::new_beneath(&self.root, &self.directory)
                .map_err(|error| publication_failed("create endpoint temporary file", error))?;
        let record = EndpointRecord {
            web_socket_url: endpoint.web_socket_url(),
            endpoint_instance: endpoint.identity().instance().to_owned(),
            process_id: endpoint.identity().process_id(),
            runtime_fingerprint: managed.map(|value| value.fingerprint().to_owned()),
            service_generation: managed.map(|value| value.generation().to_owned()),
            runtime_instance: managed.map(|value| value.endpoint().instance().to_owned()),
            runtime_process_id: managed.map(|value| value.endpoint().process_id()),
        };
        serde_json::to_writer(file.as_file_mut(), &record)
            .map_err(|error| publication_failed("write endpoint record", io::Error::from(error)))?;
        file.as_file()
            .sync_all()
            .map_err(|error| publication_failed("sync endpoint temporary file", error))?;
        file.persist_beneath(&self.directory.join(ENDPOINT_FILE))
            .map_err(|error| publication_failed("replace endpoint record", error))?;
        nessa_local_storage::sync_directory_beneath(&self.root, &self.directory)
            .map_err(|error| publication_failed("sync endpoint directory", error))
    }
}

fn publication_failed(operation: &'static str, error: io::Error) -> io::Error {
    tracing::error!(
        operation,
        error.kind = ?error.kind(),
        error.raw_os_code = ?error.raw_os_error(),
        error = %error,
        "gateway endpoint publication failed",
    );
    error
}

/// Private endpoint record plus bounded unauthenticated health correlation.
///
/// `root` must already be a private directory owned by the current OS user;
/// `directory` is a relative namespace resolved by the caller.
pub struct FileEndpointDiscovery {
    root: PathBuf,
    directory: PathBuf,
}

impl FileEndpointDiscovery {
    pub fn new(root: PathBuf, directory: PathBuf) -> Self {
        Self { root, directory }
    }

    /// Read and health-check the complete managed endpoint advertisement.
    ///
    /// Native service adapters use the managed identity to correlate the OS
    /// process with the portable runtime. Ordinary clients consume the narrower
    /// [`EndpointDiscovery`] result below.
    pub fn discover_advertisement(&self) -> io::Result<Option<GatewayEndpointAdvertisement>> {
        let path = self.directory.join(ENDPOINT_FILE);
        let mut file = match nessa_local_storage::open_beneath(
            &self.root,
            &path,
            nessa_local_storage::OpenMode::ReadNonblocking,
        ) {
            Ok(file) => file,
            Err(error) if nessa_local_storage::is_unsafe_file(&error) => return Err(error),
            Err(_) => return Ok(None),
        };
        let mut bytes = Vec::new();
        if Read::by_ref(&mut file)
            .take(16_385)
            .read_to_end(&mut bytes)
            .is_err()
        {
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
        let advertisement = record_advertisement(&record)?;
        let address = advertisement_address(advertisement.endpoint())?;
        let health = read_health(address)?;
        if !health.matches(&advertisement) {
            return Err(io::Error::new(
                ErrorKind::PermissionDenied,
                "published gateway endpoint belongs to a different process",
            ));
        }
        Ok(Some(advertisement))
    }
}

impl EndpointDiscovery for FileEndpointDiscovery {
    fn discover(&self) -> io::Result<Option<GatewayEndpoint>> {
        self.discover_advertisement()
            .map(|value| value.map(|advertisement| advertisement.endpoint().clone()))
    }
}

fn invalid_record() -> io::Error {
    io::Error::new(
        ErrorKind::InvalidData,
        "published gateway endpoint is malformed",
    )
}

fn record_advertisement(record: &EndpointRecord) -> io::Result<GatewayEndpointAdvertisement> {
    let identity = EndpointIdentity::new(record.endpoint_instance.clone(), record.process_id)
        .map_err(|_| invalid_record())?;
    let endpoint = GatewayEndpoint::new(record.web_socket_url.clone(), identity)
        .map_err(|_| invalid_record())?;
    let managed = [
        record.runtime_fingerprint.is_some(),
        record.service_generation.is_some(),
        record.runtime_instance.is_some(),
        record.runtime_process_id.is_some(),
    ];
    if managed.iter().any(|value| *value) && !managed.iter().all(|value| *value) {
        return Err(invalid_record());
    }
    let managed = if let (Some(fingerprint), Some(generation), Some(instance), Some(process_id)) = (
        &record.runtime_fingerprint,
        &record.service_generation,
        &record.runtime_instance,
        record.runtime_process_id,
    ) {
        Some(
            ManagedRuntimeIdentity::new(
                fingerprint.clone(),
                generation.clone(),
                instance.clone(),
                process_id,
            )
            .map_err(|_| invalid_record())?,
        )
    } else {
        None
    };
    GatewayEndpointAdvertisement::new(endpoint, managed).map_err(|_| invalid_record())
}

fn advertisement_address(endpoint: &GatewayEndpoint) -> io::Result<SocketAddr> {
    let web_socket_url = endpoint.web_socket_url();
    let url = Url::parse(&web_socket_url).map_err(|_| invalid_record())?;
    let ip = match url.host().ok_or_else(invalid_record)? {
        Host::Ipv4(value) => IpAddr::V4(value),
        Host::Ipv6(value) => IpAddr::V6(value),
        Host::Domain(_) => return Err(invalid_record()),
    };
    let port = url.port_or_known_default().ok_or_else(invalid_record)?;
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
    fn matches(&self, advertisement: &GatewayEndpointAdvertisement) -> bool {
        let endpoint = advertisement.endpoint().identity();
        let managed = advertisement.managed();
        let runtime_process_id = managed.map(|value| value.endpoint().process_id().to_string());
        self.endpoint_instance == endpoint.instance()
            && self.endpoint_process_id == endpoint.process_id().to_string()
            && self.runtime_fingerprint.as_deref()
                == managed.map(ManagedRuntimeIdentity::fingerprint)
            && self.service_generation.as_deref() == managed.map(ManagedRuntimeIdentity::generation)
            && self.runtime_instance.as_deref() == managed.map(|value| value.endpoint().instance())
            && self.runtime_process_id.as_ref() == runtime_process_id.as_ref()
    }
}

fn read_health(address: SocketAddr) -> io::Result<HealthIdentity> {
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut stream = TcpStream::connect_timeout(
        &address,
        remaining(deadline)?.min(Duration::from_millis(300)),
    )?;
    stream.set_write_timeout(Some(remaining(deadline)?))?;
    stream.write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")?;
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 1024];
    while bytes.len() < 8192 && !bytes.windows(4).any(|value| value == b"\r\n\r\n") {
        stream.set_read_timeout(Some(remaining(deadline)?))?;
        let capacity = chunk.len().min(8192 - bytes.len());
        let read = stream.read(&mut chunk[..capacity])?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    parse_health(&bytes).ok_or_else(|| {
        io::Error::new(
            ErrorKind::PermissionDenied,
            "published gateway endpoint health is malformed",
        )
    })
}

fn remaining(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|value| !value.is_zero())
        .ok_or_else(|| {
            io::Error::new(
                ErrorKind::TimedOut,
                "gateway endpoint health deadline elapsed",
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        error::Error,
        fmt,
        sync::{Arc, Mutex, PoisonError},
    };

    fn endpoint(web_socket_url: &str) -> GatewayEndpoint {
        GatewayEndpoint::new(
            web_socket_url.to_owned(),
            EndpointIdentity::new("7a653268-43fc-4e76-a4d3-df749cc629b1".into(), 123).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn advertisement_address_projects_validated_default_and_custom_ports() {
        for (url, expected) in [
            ("ws://127.0.0.1:80", "127.0.0.1:80"),
            ("ws://[::1]:80", "[::1]:80"),
            ("ws://127.0.0.1:9137", "127.0.0.1:9137"),
        ] {
            assert_eq!(
                advertisement_address(&endpoint(url)).unwrap(),
                expected.parse::<SocketAddr>().unwrap()
            );
        }
    }

    #[test]
    fn publication_failure_names_its_operation_and_preserves_its_source() {
        let captured = Capture(Arc::new(Mutex::new(Vec::new())));
        let subscriber = tracing_subscriber::fmt()
            .with_writer(captured.clone())
            .with_ansi(false)
            .finish();
        let temporary = tempfile::tempdir().unwrap();
        let missing_root = temporary.path().join("missing-root");
        let publication = FileEndpointPublication::new(missing_root, "logs".into());
        let endpoint = endpoint("ws://127.0.0.1:9137");
        let advertisement = GatewayEndpointAdvertisement::new(endpoint, None).unwrap();

        let (error, returned) = tracing::subscriber::with_default(subscriber, || {
            let error = publication.publish(&advertisement).unwrap_err();
            let source = io::Error::new(ErrorKind::PermissionDenied, SourceRoot(SourceMarker));
            (error, publication_failed("replace endpoint record", source))
        });
        let output = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();

        assert_eq!(error.kind(), ErrorKind::NotFound);
        assert!(error.raw_os_error().is_some());
        assert!(
            output.contains("gateway endpoint publication failed"),
            "{output}"
        );
        assert!(
            output.contains("operation=\"create endpoint directory\""),
            "{output}"
        );
        assert!(output.contains("error.kind=NotFound"), "{output}");
        assert!(output.contains("error.raw_os_code=Some("), "{output}");
        assert_eq!(returned.kind(), ErrorKind::PermissionDenied);
        assert!(returned
            .get_ref()
            .and_then(|source| source.downcast_ref::<SourceRoot>())
            .is_some());
        assert!(Error::source(&returned)
            .and_then(|source| source.downcast_ref::<SourceMarker>())
            .is_some());
    }

    #[derive(Debug)]
    struct SourceRoot(SourceMarker);

    impl fmt::Display for SourceRoot {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("source root")
        }
    }

    impl Error for SourceRoot {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            Some(&self.0)
        }
    }

    #[derive(Debug)]
    struct SourceMarker;

    impl fmt::Display for SourceMarker {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("source marker")
        }
    }

    impl Error for SourceMarker {}

    #[derive(Clone)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl Write for Capture {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Capture {
        type Writer = Self;

        fn make_writer(&'a self) -> Self {
            self.clone()
        }
    }
}
