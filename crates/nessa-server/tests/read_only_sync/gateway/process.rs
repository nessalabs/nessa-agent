//! Supplemental real gateway process: existing authenticated #297 router/source fixture.
//! Durable example cache and private local credential provisioning are composed separately.
use super::super::{
    session::{LocalConnector, Session},
    sources::GatewayConnection,
};
use crate::product_contract::generated::CatalogueReadErrorCode;
use crate::{
    app::ports::Clock,
    read_only_sync::application::{Cancellation, GatewayError, GatewayPolicy},
};
use nessa_gateway_endpoint::domain::{EndpointIdentity, GatewayEndpoint};
use nessa_sync::replication::{
    application::{Access, ScopeAuthorizer},
    catalogue::{CataloguePass, CatalogueSource, ManifestRequest},
    domain::Id,
};
use std::{
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    sync::Arc,
    time::Instant,
};
struct Time(Instant);
impl Clock for Time {
    fn elapsed_ms(&self) -> u64 {
        self.0.elapsed().as_millis() as u64
    }
}
struct Never;
impl Cancellation for Never {
    fn cancelled(&self) -> bool {
        false
    }
}
struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[test]
fn actual_authenticated_gateway_process_discovery_manifest_resolve_and_stale_epoch() {
    let directory = tempfile::tempdir().unwrap();
    let mut gateway = Process(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "product::socket::tests::catalogue_receiver::gateway_child",
                "--nocapture",
            ])
            .env("NESSA_297_GATEWAY_ROOT", directory.path().join("gateway"))
            .env("NESSA_297_SEED_COUNT", "2")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let mut output = BufReader::new(gateway.0.stdout.take().unwrap());
    let mut line = String::new();
    let port = loop {
        line.clear();
        assert_ne!(output.read_line(&mut line).unwrap(), 0);
        if let Some(port) = line.trim().strip_prefix("NESSA_297_READY ") {
            break port.parse::<u16>().unwrap();
        }
    };
    let endpoint = GatewayEndpoint::new(
        format!("ws://127.0.0.1:{port}"),
        EndpointIdentity::new("018fa012-2222-7222-8222-123456789abc".into(), 1).unwrap(),
    )
    .unwrap();
    let open = || {
        GatewayConnection::new(
            Session::connect(
                &endpoint,
                "alice-phone",
                "read-only-example",
                &LocalConnector,
                Arc::new(Time(Instant::now())),
                Arc::new(Never),
                GatewayPolicy::new(5000, 5000, 8192, 4, 4).unwrap(),
            )
            .unwrap(),
        )
    };
    let connection = open();
    let mut source = connection.catalogue(Id::new("alice-receiver").unwrap(), 7);
    let discovery = connection.run(|| source.discover()).unwrap();
    assert_eq!(discovery.outcome.failure, None);
    let (scope, head) = discovery.result.unwrap().unwrap();
    assert_eq!(scope.origin().as_str(), "gateway-resource");
    assert!(head > 0);
    let mut authorizer = source.authorizer();
    let access = connection.run(|| authorizer.authorize(&scope)).unwrap();
    assert_eq!(access.result, Some(Access::Allowed(scope.clone())));
    assert_eq!(access.outcome.failure, None);
    let pass = CataloguePass {
        scope,
        completed: 0,
        boundary: head,
        cursor: None,
        generation: 1,
    };
    let page = connection
        .run(|| {
            source.manifest(&ManifestRequest {
                pass: pass.clone(),
                max_entries: 1,
            })
        })
        .unwrap();
    assert_eq!(page.outcome.failure, None);
    let page = page.result.unwrap().unwrap();
    assert_eq!(page.entries.len(), 1);
    let resolved = connection
        .run(|| source.resolve(&pass, &page.entries[0].key.id, 65536))
        .unwrap();
    assert_eq!(resolved.outcome.failure, None);
    assert!(!resolved.result.unwrap().unwrap().payload.is_empty());
    drop(source);
    drop(authorizer);
    drop(connection);
    let connection = open();
    let mut wrong = connection.catalogue(Id::new("alice-receiver").unwrap(), 8);
    let refusal = connection.run(|| wrong.discover()).unwrap();
    assert!(refusal.result.unwrap().is_err());
    assert_eq!(
        refusal.outcome.failure,
        Some(GatewayError::Catalogue(CatalogueReadErrorCode::StaleEpoch))
    );
    drop(wrong);
    drop(connection);
    drop(gateway);
}
