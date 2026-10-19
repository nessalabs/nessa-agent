//! Public native enrollment constructors, actual peers and interrupted neighbors.
#[path = "device_pairing/infrastructure/activation.rs"]
mod activation;
#[path = "device_pairing/infrastructure/enrollment.rs"]
mod enrollment;
#[path = "device_pairing/infrastructure/framing.rs"]
mod framing;
#[path = "device_pairing/infrastructure/lifecycle.rs"]
mod lifecycle;
#[path = "device_pairing/infrastructure/listener.rs"]
mod listener;
#[cfg(unix)]
#[path = "device_pairing/mounted.rs"]
mod mounted;
/// The one-thread probe's test-only cancellation/cleanup owner and regressions.
#[path = "device_pairing/infrastructure/one_thread.rs"]
mod one_thread;
#[path = "device_pairing/owner_routes.rs"]
mod owner_routes;
#[path = "peer_gateways/enroll.rs"]
mod peer_enroll;
#[path = "device_pairing/product_client.rs"]
mod product_client;
#[path = "device_pairing/infrastructure/protected.rs"]
mod protected;
#[path = "device_pairing/infrastructure/support.rs"]
mod support;

#[path = "device_pairing/infrastructure/receiver_dispatch.rs"]
mod receiver_dispatch;
