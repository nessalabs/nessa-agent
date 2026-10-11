//! Every operational limit the gateway can hit, named once.
//!
//! This module states no number of its own. A count, a duration, or a byte
//! bound is read from the owner that already enforces it: the tier-3 value
//! [`OperationalLimits`], a fixed socket constant, or a bound the protocol
//! schema publishes. `docs/limits.md` is that catalogue rendered, and the
//! test beside this module refuses a copy that has drifted.
use crate::conversation::application::MAX_READ_GRANTS_PER_CONVERSATION;
use crate::conversation::infrastructure::{DISCOVERY_STEPS_PER_READ, MAX_CATALOGUE_CHANGE_WATCHES};
use crate::mcp_servers::infrastructure::{MAX_BUILT_IN_CALLS, MAX_BUILT_IN_LINE_BYTES};
use crate::product::passive_read::deadlines::RECORD_SEND_TIMEOUT;
use crate::product::{OperationalLimits, SessionSettings, RECORD_LANE, RECORD_SLOT, REFUSAL_LANE};
use nessa_protocol::lease::{COMMAND_STOP_WAIT, MAX_DATA_BYTES};
use nessa_protocol::product::generated::{
    MAX_CONNECTION_CONVERSATION_SUBSCRIPTIONS, MAX_CONNECTION_LIST_SUBSCRIPTIONS,
    MAX_PRODUCT_CLIENT_ID_CHARACTERS, MAX_PRODUCT_SURFACE_INSTANCE_CHARACTERS,
    MAX_RECORD_RESPONSE_BYTES, SUBSCRIPTION_DELIVERY_TIMEOUT_MS,
};
use nessa_protocol::protocol::MAX_PAYLOAD_BYTES;
use nessa_sdk::domain::agent_execution::leases::{CommandOutput, CommandWork, Lease};
use nessa_sdk::infrastructure::session_storage::MAX_RECORD_CHANGE_WATCHES;
use std::collections::BTreeMap;
use std::time::Duration;

/// One named limit: where it is enforced, and what hitting it means.
#[cfg(test)]
struct Limit {
    id: &'static str,
    tier: &'static str,
    owner: &'static str,
    meaning: &'static str,
}

/// The catalogue, in the order the rendered document lists it.
#[cfg(test)]
fn catalogue() -> &'static [Limit] {
    &[
        Limit {
            id: "product.max_payload_bytes",
            tier: "fixed",
            owner: "protocol/product/v1.json x-frameBytes.maxPayloadBytes",
            meaning: "a request frame longer than this is refused before it is decoded",
        },
        Limit {
            id: "product.max_record_response_bytes",
            tier: "fixed",
            owner: "protocol/product/v1.json RecordPageResult bounds",
            meaning: "a record page past this is refused rather than written to the socket",
        },
        Limit {
            id: "product.max_client_id_characters",
            tier: "fixed",
            owner: "protocol/product/v1.json ProductClientMetadata.id maxLength",
            meaning: "a client id longer than this is refused at the handshake",
        },
        Limit {
            id: "product.max_surface_instance_characters",
            tier: "fixed",
            owner: "protocol/product/v1.json ProductSurface.instance maxLength",
            meaning: "a surface instance longer than this is refused at the handshake",
        },
        Limit {
            id: "server.requests",
            tier: "configured",
            owner: "config.json limits.requests",
            meaning: "an ordinary request past the gateway's admission is refused",
        },
        Limit {
            id: "server.controls",
            tier: "configured",
            owner: "config.json limits.controls",
            meaning: "a control past the gateway's admission is refused",
        },
        Limit {
            id: "server.record_reads",
            tier: "configured",
            owner: "config.json limits.recordReads",
            meaning: "a record read past the gateway's admission is refused",
        },
        Limit {
            id: "server.deletions",
            tier: "configured",
            owner: "config.json limits.deletions",
            meaning: "a deletion past the gateway's admission is refused",
        },
        Limit {
            id: "server.upload_begins",
            tier: "configured",
            owner: "config.json limits.uploadBegins",
            meaning: "an upload past the gateway's admission is refused",
        },
        Limit {
            id: "socket.ordinary_slots",
            tier: "configured",
            owner: "config.json limits.ordinarySlots",
            meaning: "an ordinary frame past this socket's slots is refused",
        },
        Limit {
            id: "socket.control_slots",
            tier: "configured",
            owner: "config.json limits.controlSlots",
            meaning: "a control frame past this socket's slots is refused",
        },
        Limit {
            id: "socket.record_slot",
            tier: "fixed",
            owner: "product socket RECORD_SLOT",
            meaning: "a second record frame while one is in flight is refused",
        },
        Limit {
            id: "socket.app_calls",
            tier: "configured",
            owner: "config.json limits.appCallsPerSocket",
            meaning: "an app call past this socket's lane is refused",
        },
        Limit {
            id: "socket.app_mount",
            tier: "configured",
            owner: "one less than limits.appCallsPerSocket",
            meaning: "a mount that would take the lane's last slot is refused",
        },
        Limit {
            id: "socket.ordinary_lane",
            tier: "derived",
            owner: "ordinary slots plus app calls per socket",
            meaning: "a response that does not fit the ordinary lane closes the socket",
        },
        Limit {
            id: "socket.control_lane",
            tier: "derived",
            owner: "control slots",
            meaning: "a control response that does not fit the control lane closes the socket",
        },
        Limit {
            id: "socket.record_lane",
            tier: "fixed",
            owner: "product socket RECORD_LANE",
            meaning: "a second record response while one is queued closes the socket",
        },
        Limit {
            id: "socket.refusal_lane",
            tier: "fixed",
            owner: "product socket REFUSAL_LANE",
            meaning: "an app refusal waits while one is queued; a non-app refusal on a full lane closes the socket",
        },
        Limit {
            id: "socket.write_timeout",
            tier: "configured",
            owner: "config.json session.writeTimeoutMs",
            meaning: "a write that outlasts this closes the socket",
        },
        Limit {
            id: "socket.record_delivery_deadline",
            tier: "fixed",
            owner: "RECORD_SEND_TIMEOUT, the schema's passive delivery budget",
            meaning: "a record response still queued at this deadline is noted and dropped",
        },
        Limit {
            id: "socket.watch_delivery_deadline",
            tier: "fixed",
            owner: "RECORD_SEND_TIMEOUT, the same duration as record delivery",
            meaning: "a watch frame still queued at this deadline is noted and dropped",
        },
        Limit {
            id: "socket.subscription_delivery_deadline",
            tier: "fixed",
            owner: "protocol/product/v1.json x-subscriptionLimits.deliveryTimeoutMs",
            meaning: "a subscription frame the writer has not taken by this ends that subscription as lagging; its end frame unwritten by this closes the socket",
        },
        Limit {
            id: "socket.conversation_subscriptions",
            tier: "fixed",
            owner: "protocol/product/v1.json x-subscriptionLimits.conversationTargets",
            meaning: "a conversation subscription past this on one socket is refused",
        },
        Limit {
            id: "socket.list_subscriptions",
            tier: "fixed",
            owner: "protocol/product/v1.json x-subscriptionLimits.listTargets",
            meaning: "a list subscription past this on one socket is refused",
        },
        Limit {
            id: "record.change_watches",
            tier: "fixed",
            owner: "MAX_RECORD_CHANGE_WATCHES in crates/nessa-sdk/src/infrastructure/session_storage/record_changes.rs",
            meaning: "the record watches every socket shares, devices' and subscriptions' alike; past this a watch or subscription is refused subscription_capacity. A view subscription holds one and a list subscription one (any commit), so a desktop window following its limit (8 views and the list) holds 9, and about 7 such windows fill it",
        },
        Limit {
            id: "conversation.catalogue_watches",
            tier: "fixed",
            owner: "MAX_CATALOGUE_CHANGE_WATCHES in crates/nessa-server/src/conversation/infrastructure/catalogue_changes.rs",
            meaning: "the catalogue watches every socket shares; a list subscription holds one beside its record watch, as a device's catalogue watch does; past this one is refused subscription_capacity",
        },
        Limit {
            id: "conversation.read_grants",
            tier: "fixed",
            owner: "MAX_READ_GRANTS_PER_CONVERSATION in crates/nessa-server/src/conversation/application/read_grants.rs, the schema's ConversationSharesResult maxItems",
            meaning: "the paired devices one conversation is shared with; one more conversation.share is refused invalid_request, so a conversation.shares answer always fits one frame",
        },
        Limit {
            id: "record.read_work_budget",
            tier: "configured",
            owner: "config.json limits.readWorkBudgetMs",
            meaning: "a cold read still preparing at this budget answers that it is preparing",
        },
        Limit {
            id: "record.discovery_steps",
            tier: "fixed",
            owner: "record read DISCOVERY_STEPS_PER_READ",
            meaning:
                "a cold read that uses every discovery step answers that the source is preparing",
        },
        Limit {
            id: "command.default_timeout_ms",
            tier: "fixed",
            owner: "CommandWork::DEFAULT_TIMEOUT_MS",
            meaning: "a `run` naming no timeout is stopped after this, as timed out",
        },
        Limit {
            id: "command.max_timeout_ms",
            tier: "fixed",
            owner: "CommandWork::MAX_TIMEOUT_MS",
            meaning: "a `run` asking a longer timeout is refused before anything is recorded",
        },
        Limit {
            id: "command.stop_wait_ms",
            tier: "fixed",
            owner: "lease protocol COMMAND_STOP_WAIT",
            meaning: "a host that has not answered a command this long past its timeout, or past its stop, leaves it unanswered",
        },
        Limit {
            id: "command.capture_bytes",
            tier: "fixed",
            owner: "lease protocol MAX_DATA_BYTES",
            meaning: "a command's output past this, both streams together, is dropped from its start and counted",
        },
        Limit {
            id: "command.retained_tail_bytes",
            tier: "fixed",
            owner: "CommandOutput::MAX_TAIL_BYTES",
            meaning: "of each stream, only the newest this many bytes are kept in the command's record",
        },
        Limit {
            id: "command.live_per_lease",
            tier: "fixed",
            owner: "Lease::MAX_LIVE_COMMANDS",
            meaning: "a `run` while this many run under the agent's lease is refused `budget_exceeded`",
        },
        Limit {
            id: "command.max_args",
            tier: "fixed",
            owner: "CommandWork::MAX_ARGS",
            meaning: "a `run` with more arguments is refused before anything is recorded",
        },
        Limit {
            id: "command.max_argv_bytes",
            tier: "fixed",
            owner: "CommandWork::MAX_ARGV_BYTES",
            meaning: "a `run` whose arguments together are longer is refused before anything is recorded",
        },
        Limit {
            id: "command.max_cwd_bytes",
            tier: "fixed",
            owner: "CommandWork::MAX_CWD_BYTES",
            meaning: "a `run` whose working directory is longer is refused before anything is recorded",
        },
        Limit {
            id: "mcp.built_in_calls",
            tier: "fixed",
            owner: "built-in MCP server MAX_BUILT_IN_CALLS",
            meaning: "a tool call past this many running on one stand-in's connection is answered with an error",
        },
        Limit {
            id: "mcp.built_in_line_bytes",
            tier: "fixed",
            owner: "built-in MCP server MAX_BUILT_IN_LINE_BYTES",
            meaning: "a message longer than this ends the stand-in's connection",
        },
    ]
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn count(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

/// Effective server values, keyed by limit id, in sorted order.
pub(crate) fn effective_json(
    limits: &OperationalLimits,
    session: &SessionSettings,
) -> serde_json::Value {
    let mut values = BTreeMap::new();
    let mut put = |id: &str, value: u64| {
        values.insert(id.to_owned(), serde_json::Value::from(value));
    };
    put("product.max_payload_bytes", MAX_PAYLOAD_BYTES as u64);
    put(
        "product.max_record_response_bytes",
        count(MAX_RECORD_RESPONSE_BYTES),
    );
    put(
        "product.max_client_id_characters",
        count(MAX_PRODUCT_CLIENT_ID_CHARACTERS),
    );
    put(
        "product.max_surface_instance_characters",
        count(MAX_PRODUCT_SURFACE_INSTANCE_CHARACTERS),
    );
    put("server.requests", count(limits.requests()));
    put("server.controls", count(limits.controls()));
    put("server.record_reads", count(limits.record_reads()));
    put("server.deletions", count(limits.deletions()));
    put("server.upload_begins", count(limits.upload_begins()));
    put("socket.ordinary_slots", count(limits.ordinary_slots()));
    put("socket.control_slots", count(limits.control_slots()));
    put("socket.record_slot", count(RECORD_SLOT));
    put("socket.app_calls", count(limits.app_calls_per_socket()));
    put("socket.app_mount", count(limits.app_calls_per_mount()));
    put("socket.ordinary_lane", count(limits.ordinary_lane()));
    put("socket.control_lane", count(limits.control_lane()));
    put("socket.record_lane", count(RECORD_LANE));
    put("socket.refusal_lane", count(REFUSAL_LANE));
    put("socket.write_timeout", millis(session.write_timeout()));
    put(
        "socket.record_delivery_deadline",
        millis(RECORD_SEND_TIMEOUT),
    );
    put(
        "socket.watch_delivery_deadline",
        millis(RECORD_SEND_TIMEOUT),
    );
    put(
        "socket.subscription_delivery_deadline",
        SUBSCRIPTION_DELIVERY_TIMEOUT_MS,
    );
    put(
        "socket.conversation_subscriptions",
        count(MAX_CONNECTION_CONVERSATION_SUBSCRIPTIONS),
    );
    put(
        "socket.list_subscriptions",
        count(MAX_CONNECTION_LIST_SUBSCRIPTIONS),
    );
    put("record.change_watches", count(MAX_RECORD_CHANGE_WATCHES));
    put(
        "conversation.catalogue_watches",
        count(MAX_CATALOGUE_CHANGE_WATCHES),
    );
    put(
        "conversation.read_grants",
        MAX_READ_GRANTS_PER_CONVERSATION.unsigned_abs(),
    );
    put("record.read_work_budget", millis(limits.read_work_budget()));
    put("record.discovery_steps", count(DISCOVERY_STEPS_PER_READ));
    put(
        "command.default_timeout_ms",
        CommandWork::DEFAULT_TIMEOUT_MS,
    );
    put("command.max_timeout_ms", CommandWork::MAX_TIMEOUT_MS);
    put("command.stop_wait_ms", millis(COMMAND_STOP_WAIT));
    put("command.capture_bytes", count(MAX_DATA_BYTES));
    put(
        "command.retained_tail_bytes",
        count(CommandOutput::MAX_TAIL_BYTES),
    );
    put("command.live_per_lease", count(Lease::MAX_LIVE_COMMANDS));
    put("command.max_args", count(CommandWork::MAX_ARGS));
    put("command.max_argv_bytes", count(CommandWork::MAX_ARGV_BYTES));
    put("command.max_cwd_bytes", count(CommandWork::MAX_CWD_BYTES));
    put("mcp.built_in_calls", count(MAX_BUILT_IN_CALLS));
    put("mcp.built_in_line_bytes", count(MAX_BUILT_IN_LINE_BYTES));
    serde_json::Value::Object(values.into_iter().collect())
}

#[cfg(test)]
fn value_of(id: &str, values: &serde_json::Map<String, serde_json::Value>) -> String {
    values
        .get(id)
        .and_then(serde_json::Value::as_u64)
        .map(|value| value.to_string())
        .unwrap_or_else(|| "missing".to_owned())
}

/// The catalogue rendered as the committed `docs/limits.md`.
#[cfg(test)]
fn render(limits: &OperationalLimits, session: &SessionSettings) -> String {
    let values = match effective_json(limits, session) {
        serde_json::Value::Object(values) => values,
        _ => serde_json::Map::new(),
    };
    let mut document = String::from(
        "# Operational limits\n\n\
         Each row is a limit the gateway names when a refusal or a silent close hits it. \
         The number is the owner's, read when this document is rendered; this file is compared \
         to that rendering and is not edited by hand. \
         The client's preparing retries are named here and counted only in \
         `PREPARING_ATTEMPTS` (`crates/nessa-client-core/src/read_only_sync/application/watch.rs`); \
         this document does not copy that count. \
         A wire field carrying the limit id is a later schema change and is not these rows.\n\n\
         | id | tier | effective | owner | what hitting it means |\n\
         | --- | --- | --- | --- | --- |\n",
    );
    for limit in catalogue() {
        document.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            limit.id,
            limit.tier,
            value_of(limit.id, &values),
            limit.owner,
            limit.meaning,
        ));
    }
    document.push_str(
        "\n| client.preparing_attempts | client | the client's own | \
         `PREPARING_ATTEMPTS` in `crates/nessa-client-core/src/read_only_sync/application/watch.rs` | \
         how many times a preparing read is asked again |\n",
    );
    document
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Turn CRLF into LF. A Windows checkout may do the reverse to the
    /// committed file; the rendering, and what `UPDATE_LIMITS_DOC=1` writes,
    /// stay LF.
    fn lf_newlines(text: &str) -> String {
        text.replace("\r\n", "\n")
    }

    #[test]
    fn the_rendered_catalogue_matches_the_committed_document() {
        let limits = OperationalLimits::default();
        let session = SessionSettings::default();
        let rendered = render(&limits, &session);
        assert!(
            !rendered.contains('\r'),
            "UPDATE_LIMITS_DOC writes this rendering, and it is LF"
        );
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/limits.md");
        if std::env::var_os("UPDATE_LIMITS_DOC").is_some() {
            std::fs::write(path, &rendered).expect("rewrite docs/limits.md");
        }
        let committed = std::fs::read_to_string(path).unwrap_or_default();
        assert_eq!(
            lf_newlines(&committed),
            rendered,
            "docs/limits.md drifted; regenerate with UPDATE_LIMITS_DOC=1"
        );
    }

    #[test]
    fn a_crlf_checkout_of_the_catalogue_is_not_drift() {
        let rendered = render(&OperationalLimits::default(), &SessionSettings::default());
        let checked_out = rendered.replace('\n', "\r\n");
        assert_ne!(checked_out, rendered);
        assert_eq!(lf_newlines(&checked_out), rendered);
    }

    #[test]
    fn the_client_row_names_the_constant_and_does_not_copy_its_count() {
        let rendered = render(&OperationalLimits::default(), &SessionSettings::default());
        let row = rendered
            .lines()
            .find(|line| line.contains("client.preparing_attempts"))
            .expect("client row");
        assert!(row.contains("PREPARING_ATTEMPTS"));
        assert!(row.contains("the client's own"));
        assert!(!row.contains("| 4 |"));
    }

    #[test]
    fn effective_values_follow_the_owners() {
        let limits = OperationalLimits::default();
        let session = SessionSettings::default();
        let document = effective_json(&limits, &session);
        let values = document.as_object().expect("object");
        assert_eq!(
            values.get("server.requests").and_then(|v| v.as_u64()),
            Some(count(limits.requests()))
        );
        assert_eq!(
            values
                .get("record.read_work_budget")
                .and_then(|v| v.as_u64()),
            Some(millis(limits.read_work_budget()))
        );
        assert_eq!(
            values.get("socket.record_slot").and_then(|v| v.as_u64()),
            Some(count(RECORD_SLOT))
        );
        for limit in catalogue() {
            assert!(
                values.contains_key(limit.id),
                "{} is in the catalogue and not in the effective values",
                limit.id
            );
        }
        let keys: Vec<_> = values.keys().cloned().collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
    }
}
