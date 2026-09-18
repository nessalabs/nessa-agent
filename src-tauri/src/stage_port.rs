//! Stage → loopback port for the background gateway this host registers.
//!
//! The table is `protocol/defaults/gateway-ports.json`, the same bytes
//! `nessa-server` compiles in and the frontend imports. The host does not pick
//! a port of its own: it writes the stage's port into the service definition as
//! `NESSA_PORT` and probes that same port for health, so the registration and
//! the probe can never name different sockets.
//!
//! A packaged build registers a `prod` service and therefore keeps 7420. A
//! stage the table does not name has no port, and registration says so rather
//! than quietly taking the product's socket.

use std::collections::BTreeMap;

use serde::Deserialize;

const PORTS_JSON: &str = include_str!("../../protocol/defaults/gateway-ports.json");

#[derive(Debug, Deserialize)]
struct GatewayPorts {
    stages: BTreeMap<String, u16>,
}

/// Port the gateway listens on for `stage`, or `None` when the table has no
/// entry — the same stages `nessa-server` itself accepts.
pub fn stage_port(stage: &str) -> Option<u16> {
    let table: GatewayPorts =
        serde_json::from_str(PORTS_JSON).expect("bundled gateway-ports.json must parse");
    table.stages.get(stage).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packaged_prod_service_keeps_the_product_port() {
        assert_eq!(stage_port("prod"), Some(7420));
    }

    #[test]
    fn dev_listens_beside_the_product_port() {
        assert_eq!(stage_port("dev"), Some(7421));
    }

    #[test]
    fn an_unnamed_stage_has_no_port() {
        assert_eq!(stage_port("staging"), None);
    }
}
