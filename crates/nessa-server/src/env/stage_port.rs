//! Stage → loopback port, read from `protocol/defaults/gateway-ports.json`.
//!
//! That file is the one table in the repository. The desktop host includes the
//! same bytes for the service it registers, and the frontend and the Vite proxy
//! import it, so a port cannot be right here and stale there.
//!
//! `NESSA_PORT` still wins over the table; this answers only "what does this
//! stage listen on when nothing says otherwise".

use std::collections::BTreeMap;
use std::sync::LazyLock;

use serde::Deserialize;

use super::stage::Stage;

const PORTS_JSON: &str = include_str!("../../../../protocol/defaults/gateway-ports.json");

#[derive(Debug, Deserialize)]
struct GatewayPorts {
    stages: BTreeMap<String, u16>,
}

static PORTS: LazyLock<GatewayPorts> = LazyLock::new(|| {
    serde_json::from_str(PORTS_JSON).expect("protocol/defaults/gateway-ports.json must parse")
});

/// Default listen port for `stage`. Panics only if the checked-in table has no
/// entry for a stage the server accepts; a test below covers every variant.
pub fn stage_port(stage: Stage) -> u16 {
    PORTS
        .stages
        .get(stage.as_str())
        .copied()
        .unwrap_or_else(|| {
            panic!(
                "gateway-ports.json has no port for stage {}",
                stage.as_str()
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_stage_the_server_accepts_has_a_port() {
        for stage in [Stage::Dev, Stage::Alpha, Stage::Ci, Stage::Prod] {
            assert!(stage_port(stage) > 0);
        }
    }

    /// And the other direction, which nothing checked: a stage in the table
    /// that this server would refuse.
    ///
    /// Everything else reads the table — the Tauri host's launchd registration,
    /// the Vite proxy, the `just` recipes — and each resolves a port for any key
    /// it finds there. A key this server cannot parse is therefore a port that
    /// is freed, probed and proxied for a gateway that will never start on it,
    /// which is three failures wearing different hats. The table is where a
    /// stage is added, so the table is what has to stay inside what is
    /// accepted here.
    #[test]
    fn every_stage_the_table_names_is_one_the_server_accepts() {
        let table: serde_json::Value = serde_json::from_str(PORTS_JSON)
            .expect("bundled gateway-ports.json must parse");
        let stages = table["stages"]
            .as_object()
            .expect("the table lists its stages as an object");
        assert!(!stages.is_empty(), "a table with no stages proves nothing");
        for named in stages.keys() {
            assert!(
                Stage::parse(named).is_ok(),
                "gateway-ports.json names the stage {named}, which Stage::parse refuses",
            );
        }
    }

    #[test]
    fn dev_listens_beside_the_product_port_rather_than_on_it() {
        assert_eq!(stage_port(Stage::Prod), 7420);
        assert_eq!(stage_port(Stage::Dev), 7421);
        assert_ne!(stage_port(Stage::Dev), stage_port(Stage::Prod));
    }
}
