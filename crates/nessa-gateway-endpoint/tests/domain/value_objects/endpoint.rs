use nessa_gateway_endpoint::domain::{EndpointIdentity, GatewayEndpoint};

fn identity() -> EndpointIdentity {
    EndpointIdentity::new("7a653268-43fc-4e76-a4d3-df749cc629b1".into(), 123).unwrap()
}

#[test]
fn identity_requires_a_canonical_process_incarnation() {
    assert!(EndpointIdentity::new("not-a-uuid".into(), 1).is_err());
    assert!(EndpointIdentity::new("7A653268-43FC-4E76-A4D3-DF749CC629B1".into(), 1).is_err());
    assert!(EndpointIdentity::new("7a653268-43fc-4e76-a4d3-df749cc629b1".into(), 0).is_err());
}

#[test]
fn endpoint_accepts_only_a_bound_loopback_socket() {
    let loopback = GatewayEndpoint::new("ws://127.0.0.1:9123".into(), identity()).unwrap();
    assert_eq!(loopback.web_socket_url(), "ws://127.0.0.1:9123");
    assert_eq!(loopback.socket_address(), "127.0.0.1:9123".parse().unwrap());
    let ipv6 = GatewayEndpoint::new("ws://[::1]:9123".into(), identity()).unwrap();
    assert_eq!(ipv6.socket_address(), "[::1]:9123".parse().unwrap());
    assert!(GatewayEndpoint::new("ws://127.0.0.1:0".into(), identity()).is_err());
    assert!(GatewayEndpoint::new("ws://127.0.0.1".into(), identity()).is_err());
    assert!(GatewayEndpoint::new("ws://127.0.0.1:9123/".into(), identity()).is_err());
    assert!(GatewayEndpoint::new("wss://127.0.0.1:9123".into(), identity()).is_err());
    assert!(GatewayEndpoint::new("ws://192.0.2.1:9123".into(), identity()).is_err());
    assert!(GatewayEndpoint::new("ws://127.0.0.2:9123".into(), identity()).is_err());
    assert!(GatewayEndpoint::new("ws://[::1]:9123".into(), identity()).is_ok());
}
