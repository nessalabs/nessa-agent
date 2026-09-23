use super::*;
use nessa_gateway_endpoint::{
    application::EndpointDiscovery,
    domain::{EndpointIdentity, GatewayEndpoint},
};
use std::{io, sync::Arc};

struct Fixed(Result<Option<GatewayEndpoint>, io::Error>);

impl EndpointDiscovery for Fixed {
    fn discover(&self) -> io::Result<Option<GatewayEndpoint>> {
        match &self.0 {
            Ok(value) => Ok(value.clone()),
            Err(error) => Err(io::Error::new(error.kind(), error.to_string())),
        }
    }
}

fn endpoint() -> GatewayEndpoint {
    GatewayEndpoint::new(
        "ws://127.0.0.1:9137".into(),
        EndpointIdentity::new("5485b918-1eeb-4a4a-ad1d-9fdc70dfa231".into(), 4711).unwrap(),
    )
    .unwrap()
}

#[test]
fn stage_and_requested_credential_destination_must_match_the_verified_endpoint() {
    let access = GatewayEndpointAccess::new("prod".into(), Arc::new(Fixed(Ok(Some(endpoint())))));
    assert_eq!(
        access.resolve("prod").unwrap(),
        Some("ws://127.0.0.1:9137".into())
    );
    assert!(access.resolve("dev").is_err());
    assert!(access
        .permits_credential_for("prod", "ws://127.0.0.1:9137")
        .is_ok());
    assert!(access
        .permits_credential_for("prod", "ws://127.0.0.1:7420")
        .is_err());
}

#[test]
fn malformed_or_mismatched_publication_failure_is_not_absence() {
    let access = GatewayEndpointAccess::new(
        "prod".into(),
        Arc::new(Fixed(Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "identity mismatch",
        )))),
    );
    assert!(access.resolve("prod").is_err());
    assert!(access
        .permits_credential_for("prod", "ws://127.0.0.1:7420")
        .is_err());
}
