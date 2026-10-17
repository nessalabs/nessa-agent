//! Every operational limit the gateway can hit, named once.
//!
//! This module states no number of its own. A count, a duration, or a byte
//! bound is read from the owner that already enforces it: the tier-3 value
//! [`OperationalLimits`], a fixed socket constant, or a bound the protocol
//! schema publishes. `docs/limits.md` is that catalogue rendered, and the
//! test beside this module refuses a copy that has drifted.
use crate::attachments::application::{AttachmentLimits, HeldReads};
use crate::conversation::application::{
    ArtifactBudget, ARTIFACT_READ_RESUMES, FETCH_BUSY_BACKOFF, FETCH_CALL_WAIT, FETCH_READS,
    MAX_ADMITTED_POLICIES, MAX_DRIVE_GRANTS_PER_CONVERSATION, MAX_SHARE_GRANTS_PER_CONVERSATION,
    PUBLISH_CALL_WAIT,
};
use crate::conversation::domain::{CommandHosts, ToolRules};
use crate::conversation::infrastructure::{DISCOVERY_STEPS_PER_READ, MAX_CATALOGUE_CHANGE_WATCHES};
use crate::mcp_servers::infrastructure::{MAX_BUILT_IN_CALLS, MAX_BUILT_IN_LINE_BYTES};
use crate::peer_gateways::infrastructure::{
    AUDIT_QUEUE, POLL_BACKOFF_CAP, POLL_INTERVAL, PREEMPT, READS_AT_ONCE, READ_BUDGET,
    WITHDRAWN_PER_READ,
};
use crate::product::passive_read::deadlines::RECORD_SEND_TIMEOUT;
use crate::product::{OperationalLimits, SessionSettings, RECORD_LANE, RECORD_SLOT, REFUSAL_LANE};
use nessa_protocol::lease::{
    COMMAND_STOP_WAIT, MAX_ARTIFACTS_IN_FLIGHT, MAX_ARTIFACT_BYTES, MAX_ARTIFACT_READS_IN_FLIGHT,
    MAX_DATA_BYTES, MAX_FETCHES_IN_FLIGHT, MAX_OPEN_PUBLISHERS,
};
use nessa_protocol::product::generated::{
    MAX_CONNECTION_CONVERSATION_SUBSCRIPTIONS, MAX_CONNECTION_LIST_SUBSCRIPTIONS,
    MAX_PRINCIPAL_SUBSCRIPTIONS, MAX_PRODUCT_CLIENT_ID_CHARACTERS,
    MAX_PRODUCT_SURFACE_INSTANCE_CHARACTERS, MAX_RECORD_RESPONSE_BYTES, MAX_SHARE_PAGE,
    SUBSCRIPTION_DELIVERY_TIMEOUT_MS,
};
use nessa_protocol::protocol::MAX_PAYLOAD_BYTES;
use nessa_sdk::application::agent_execution::sessions::{ArtifactRecord, NoteRecord, NoteText};
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
            id: "socket.principal_subscriptions",
            tier: "fixed",
            owner: "protocol/product/v1.json x-subscriptionLimits.principalTargets",
            meaning: "view, event and list subscriptions across one authenticated organization and principal share this allowance; refused admission takes no source watch, and task cancellation releases it after the sources",
        },
        Limit {
            id: "record.change_watches",
            tier: "fixed",
            owner: "MAX_RECORD_CHANGE_WATCHES in crates/nessa-sdk/src/infrastructure/session_storage/record_changes.rs",
            meaning: "the record watches every socket shares, devices' and subscriptions' alike; past this a watch or subscription is refused subscription_capacity. A view subscription holds one record watch and each list subscription holds one (any commit); a connection consumes a watch for each followed target",
        },
        Limit {
            id: "conversation.catalogue_watches",
            tier: "fixed",
            owner: "MAX_CATALOGUE_CHANGE_WATCHES in crates/nessa-server/src/conversation/infrastructure/catalogue_changes.rs",
            meaning: "the catalogue watches every socket shares; a list subscription holds one beside its record watch, as a device's catalogue watch does; past this one is refused subscription_capacity",
        },
        Limit {
            id: "conversation.share_grants",
            tier: "fixed",
            owner: "MAX_SHARE_GRANTS_PER_CONVERSATION in crates/nessa-server/src/conversation/application/share_grants.rs, the schema's ConversationSharesResult x-maxGrants",
            meaning: "the paired parties one conversation is shared with, any role; one more conversation.share is refused invalid_request. conversation.shares returns them a page at a time",
        },
        Limit {
            id: "conversation.share_page",
            tier: "fixed",
            owner: "MAX_SHARE_PAGE in the generated product protocol, the schema's ConversationSharesParams pageSize",
            meaning: "the grants one conversation.shares answer holds; a pageSize outside 1 through this is refused invalid_request, and a page of this many grants, each at its largest policy, fits one frame",
        },
        Limit {
            id: "conversation.drive_grants",
            tier: "fixed",
            owner: "MAX_DRIVE_GRANTS_PER_CONVERSATION in crates/nessa-server/src/conversation/application/share_grants.rs",
            meaning: "the drive grants one conversation holds; one more conversation.share naming drive is refused invalid_request",
        },
        Limit {
            id: "conversation.drive_policies",
            tier: "fixed",
            owner: "MAX_ADMITTED_POLICIES in crates/nessa-server/src/conversation/application/drive_tools.rs",
            meaning: "the Drive grant policies one open conversation keeps for the turns it admitted, by grant and revision; past this the least recently admitted is let go, and a turn still needing it has its tool calls refused, never allowed",
        },
        Limit {
            id: "conversation.notes",
            tier: "fixed",
            owner: "NoteRecord::MAX_PER_CONVERSATION in crates/nessa-sdk/src/application/agent_execution/sessions/notes.rs",
            meaning: "the notes one conversation keeps, every author's together; one more conversation.message is refused message_quota_exceeded and nothing is saved",
        },
        Limit {
            id: "conversation.note_rate",
            tier: "fixed",
            owner: "NoteRecord::MAX_PER_AUTHOR_IN_WINDOW and RATE_WINDOW_MS in crates/nessa-sdk/src/application/agent_execution/sessions/notes.rs",
            meaning: "the notes one author, a principal under one authority, leaves in one conversation in any 60 seconds on the gateway's wall clock; one more is refused message_quota_exceeded, and a clock set back makes no room",
        },
        Limit {
            id: "conversation.pending_notes",
            tier: "fixed",
            owner: "NoteRecord::MAX_PENDING in crates/nessa-sdk/src/application/agent_execution/sessions/notes.rs",
            meaning: "the next_turn notes one conversation keeps waiting for a turn, not counting one past its lifetime; one more next_turn note is refused message_quota_exceeded and nothing is saved",
        },
        Limit {
            id: "conversation.note_lifetime",
            tier: "fixed",
            owner: "NoteRecord::PENDING_LIFETIME_MS in crates/nessa-sdk/src/application/agent_execution/sessions/notes.rs",
            meaning: "how long a next_turn note waits for a turn, from when it was accepted, on the gateway's wall clock; the first turn admitted after that retires it expired",
        },
        Limit {
            id: "conversation.notes_per_turn",
            tier: "fixed",
            owner: "UserMessage::MAX_NOTES in crates/nessa-sdk/src/domain/agent_execution/prompts/value_objects/user_message.rs, which NoteRecord::MAX_PER_TURN follows",
            meaning: "the next_turn notes one turn takes, the oldest first; the rest wait for the next turn",
        },
        Limit {
            id: "conversation.note_bytes",
            tier: "fixed",
            owner: "NoteText::MAX_BYTES in crates/nessa-sdk/src/application/agent_execution/sessions/notes.rs, the schema's ConversationMessageParams",
            meaning: "the longest note's text, in UTF-8 bytes; a longer or all-blank one is refused invalid_request",
        },
        Limit {
            id: "conversation.tool_policy_rules",
            tier: "fixed",
            owner: "ToolRules::MAX_RULES in crates/nessa-server/src/conversation/domain/value_objects/share_access.rs, the schema's ConversationToolPolicy",
            meaning: "the patterns each of a drive grant's allow and deny lists holds; a conversation.share with more in either is refused invalid_request",
        },
        Limit {
            id: "conversation.tool_policy_rule_bytes",
            tier: "fixed",
            owner: "ToolRules::MAX_RULE_BYTES in crates/nessa-server/src/conversation/domain/value_objects/share_access.rs, the schema's ConversationToolPolicy",
            meaning: "the longest tool policy pattern, in UTF-8 bytes; a conversation.share with a longer one is refused invalid_request",
        },
        Limit {
            id: "conversation.tool_policy_command_hosts",
            tier: "fixed",
            owner: "CommandHosts::MAX_HOSTS in crates/nessa-server/src/conversation/domain/value_objects/share_access.rs, the schema's ConversationToolPolicy",
            meaning: "the hosts a drive grant's policy names for its turns' commands, apart from its allow and deny patterns; a conversation.share with more is refused invalid_request, and a grant naming none runs commands nowhere",
        },
        Limit {
            id: "conversation.tool_policy_command_host_bytes",
            tier: "fixed",
            owner: "CommandHosts::MAX_HOST_BYTES in crates/nessa-server/src/conversation/domain/value_objects/share_access.rs, the schema's ConversationToolPolicy",
            meaning: "the longest command host a drive grant's policy names, in bytes, the longest SSH destination config.json accepts; a conversation.share with a longer one is refused invalid_request",
        },
        Limit {
            id: "record.read_work_budget",
            tier: "configured",
            owner: "config.json limits.readWorkBudgetMs",
            meaning: "a cold read still preparing at this budget answers that it is preparing",
        },
        Limit {
            id: "artifact.max_file_bytes",
            tier: "fixed",
            owner: "nessa_protocol::lease MAX_ARTIFACT_BYTES",
            meaning: "a host refuses to publish a larger file, and the gateway to keep one",
        },
        Limit {
            id: "artifact.host_in_flight",
            tier: "fixed",
            owner: "nessa_protocol::lease MAX_ARTIFACTS_IN_FLIGHT",
            meaning: "a publish past this many unanswered under one lease is answered busy",
        },
        Limit {
            id: "artifact.paired_reads_in_flight",
            tier: "fixed",
            owner: "nessa_protocol::lease MAX_ARTIFACT_READS_IN_FLIGHT",
            meaning: "the reads of one published file an environment holds over a paired connection until each answer is handed to its writer; one more is refused busy",
        },
        Limit {
            id: "artifact.host_open_publishers",
            tier: "fixed",
            owner: "nessa_protocol::lease MAX_OPEN_PUBLISHERS",
            meaning: "a publisher connecting while this many are open waits to be accepted",
        },
        Limit {
            id: "artifact.environment_files",
            tier: "fixed",
            owner: "conversation ArtifactBudget::LEASE.files",
            meaning: "a file one lease publishes past this many kept is refused unread",
        },
        Limit {
            id: "artifact.environment_bytes",
            tier: "fixed",
            owner: "conversation ArtifactBudget::LEASE.bytes",
            meaning: "a file that would take one lease's total kept bytes past this is refused unread",
        },
        Limit {
            id: "artifact.conversation_records",
            tier: "fixed",
            owner: "nessa-sdk ArtifactRecord::MAX_PER_CONVERSATION",
            meaning: "a file published once a conversation records this many is refused unread",
        },
        Limit {
            id: "artifact.read_resumes",
            tier: "fixed",
            owner: "conversation ARTIFACT_READ_RESUMES",
            meaning: "a published file's read, or a fetched file's write, whose channel fails more often than this is refused channelUnavailable; each resumes from the offset acknowledged",
        },
        Limit {
            id: "artifact.read_range_bytes",
            tier: "fixed",
            owner: "attachments HeldReads::MAX_RANGE_BYTES, the schema's ArtifactReadParams maxBytes",
            meaning: "the most bytes one artifact.read answers with; its base64 fits the socket's ordinary frame",
        },
        Limit {
            id: "artifact.read_busy_wait",
            tier: "fixed",
            owner: "attachments HeldReads::BUSY_WAIT",
            meaning: "how long one artifact.read waits, queued in arrival order, for the store's one read slot before it answers temporarily_unavailable",
        },
        Limit {
            id: "server.artifact_reads",
            tier: "fixed",
            owner: "attachments HeldReads::CONCURRENT_READS",
            meaning: "artifact.read requests the gateway reads or waits on at once, apart from server.requests; past this one is refused temporarily_unavailable at once",
        },
        Limit {
            id: "artifact.publish_deadline",
            tier: "fixed",
            owner: "attachments AttachmentLimits::publish_deadline",
            meaning: "a published file not read and checked by then is refused, and nothing held",
        },
        Limit {
            id: "artifact.publish_call_wait",
            tier: "fixed",
            owner: "conversation PUBLISH_CALL_WAIT",
            meaning: "an artifacts_publish call whose file is not kept this long after it asked answers timed_out; counted from the ask, so it covers staging and any wait behind files already offered under the lease as well as the read; a timed_out file may still be kept, named for the call's person",
        },
        Limit {
            id: "artifact.fetch_bytes",
            tier: "fixed",
            owner: "conversation ArtifactBudget::LEASE.fetch_bytes",
            meaning: "an artifacts_fetch that would take one lease's fetched bytes past this is refused budget_exceeded before the environment is asked; a fetch's size is given back when its answer says it placed nothing, or when no answer came and every byte was never said delivered",
        },
        Limit {
            id: "artifact.fetch_call_wait",
            tier: "fixed",
            owner: "conversation FETCH_CALL_WAIT",
            meaning: "an artifacts_fetch call not answered this long after it asked answers timed_out; stopped before every byte was written, the environment places nothing, and after, it may still place the file",
        },
        Limit {
            id: "server.fetch_reads",
            tier: "fixed",
            owner: "conversation FETCH_READS",
            meaning: "held-file ranges artifacts_fetch reads at once across the gateway, apart from server.artifact_reads so fetches never lock a surface out; one more waits for a permit, within its call's wait",
        },
        Limit {
            id: "artifact.fetch_busy_backoff",
            tier: "fixed",
            owner: "conversation FETCH_BUSY_BACKOFF",
            meaning: "the longest a fetch's range waits, its read permit let go, before asking again after the store answered busy; it waits 250 then 500 ms, then this each time, within its call's wait",
        },
        Limit {
            id: "artifact.host_fetches_in_flight",
            tier: "fixed",
            owner: "CurrentLease::MAX_ASKED_FETCHES (the gateway), nessa_protocol::lease MAX_FETCHES_IN_FLIGHT (the host)",
            meaning: "a fetch past this many under way under one lease is refused busy by the gateway before the host is asked, with nothing recorded; the host refuses busy past this many unanswered as well, for any gateway",
        },
        Limit {
            id: "record.discovery_steps",
            tier: "fixed",
            owner: "record read DISCOVERY_STEPS_PER_READ",
            meaning:
                "a cold read that uses every discovery step answers that the source is preparing",
        },
        Limit {
            id: "peer.poll_interval",
            tier: "fixed",
            owner: "POLL_INTERVAL in crates/nessa-server/src/peer_gateways/infrastructure/poller.rs",
            meaning: "how often each up-to-date peer gateway is read, jittered 80 to 120 percent; what a peer shares or unshares reaches this gateway's cache within about one interval",
        },
        Limit {
            id: "peer.poll_backoff_cap",
            tier: "fixed",
            owner: "POLL_BACKOFF_CAP in crates/nessa-server/src/peer_gateways/infrastructure/poller.rs",
            meaning: "the longest wait a peer is scheduled for between reads, jitter included; a peer that keeps failing waits twice as long each time, from the interval up to this; a peer that is due while every read slot is taken waits for a slot on top of this",
        },
        Limit {
            id: "peer.read_budget",
            tier: "fixed",
            owner: "READ_BUDGET in crates/nessa-server/src/peer_gateways/infrastructure/poller.rs",
            meaning: "the longest one read of one peer gateway runs, on the injected clock; a read that reaches it having saved something ends incomplete and continues on the next cycle, and one that saved nothing is unreachable and backs off. Each peer has this timeout of its own, so a slow or failing peer is reported and the others are read without waiting it out",
        },
        Limit {
            id: "peer.preempt",
            tier: "fixed",
            owner: "PREEMPT in crates/nessa-server/src/peer_gateways/infrastructure/commands.rs",
            meaning: "the longest peer.forget or peer.relay waits for the read of the peer it names to give back that peer's turn; peer.enroll does not stop a read. At least twice the read's unstoppable connect (the 5 s handshake), and the read only queues its audit records before it gives the turn back, so past it the command answers peer_busy only if local storage stalls",
        },
        Limit {
            id: "peer.audit_queue",
            tier: "fixed",
            owner: "AUDIT_QUEUE in crates/nessa-server/src/peer_gateways/infrastructure/audit.rs",
            meaning: "the most peer audit records waiting to be written, one at a time in the order they were handed over; sized for every read running at once plus two owner records, and a further owner command waits for a place inside its 5 s audit deadline; a record past it is refused at once, its sequence left as a gap, so peer.enroll, peer.forget or peer.relay answers peer_audit_unavailable before any effect and a peer read's change stands with its record logged as not kept",
        },
        Limit {
            id: "peer.reads_at_once",
            tier: "fixed",
            owner: "READS_AT_ONCE in crates/nessa-server/src/peer_gateways/infrastructure/poller.rs",
            meaning: "how many peer gateways are read at once; one more stays due until a read finishes and frees a place, and is not failed or skipped. A slow peer holds only its own place, until that cycle ends, and its read is bounded by its own read budget",
        },
        Limit {
            id: "peer.withdrawn_per_read",
            tier: "fixed",
            owner: "WITHDRAWN_PER_READ in crates/nessa-server/src/peer_gateways/infrastructure/poller.rs",
            meaning: "the most conversations one read of a peer gateway withdraws, sixteen records of 64; a read that reaches it ends incomplete and the next cycle, soon, withdraws the rest",
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
    put(
        "socket.principal_subscriptions",
        count(MAX_PRINCIPAL_SUBSCRIPTIONS),
    );
    put("record.change_watches", count(MAX_RECORD_CHANGE_WATCHES));
    put(
        "conversation.catalogue_watches",
        count(MAX_CATALOGUE_CHANGE_WATCHES),
    );
    put(
        "conversation.share_grants",
        MAX_SHARE_GRANTS_PER_CONVERSATION.unsigned_abs(),
    );
    put("conversation.share_page", count(MAX_SHARE_PAGE));
    put(
        "conversation.drive_grants",
        MAX_DRIVE_GRANTS_PER_CONVERSATION.unsigned_abs(),
    );
    put("conversation.drive_policies", count(MAX_ADMITTED_POLICIES));
    put(
        "conversation.notes",
        count(NoteRecord::MAX_PER_CONVERSATION),
    );
    put(
        "conversation.note_rate",
        count(NoteRecord::MAX_PER_AUTHOR_IN_WINDOW),
    );
    put("conversation.pending_notes", count(NoteRecord::MAX_PENDING));
    put(
        "conversation.note_lifetime",
        millis(Duration::from_millis(NoteRecord::PENDING_LIFETIME_MS)),
    );
    put(
        "conversation.notes_per_turn",
        count(NoteRecord::MAX_PER_TURN),
    );
    put("conversation.note_bytes", count(NoteText::MAX_BYTES));
    put(
        "conversation.tool_policy_rules",
        count(ToolRules::MAX_RULES),
    );
    put(
        "conversation.tool_policy_rule_bytes",
        count(ToolRules::MAX_RULE_BYTES),
    );
    put(
        "conversation.tool_policy_command_hosts",
        count(CommandHosts::MAX_HOSTS),
    );
    put(
        "conversation.tool_policy_command_host_bytes",
        count(CommandHosts::MAX_HOST_BYTES),
    );
    put("record.read_work_budget", millis(limits.read_work_budget()));
    put("record.discovery_steps", count(DISCOVERY_STEPS_PER_READ));
    put("artifact.max_file_bytes", MAX_ARTIFACT_BYTES);
    put("artifact.host_in_flight", count(MAX_ARTIFACTS_IN_FLIGHT));
    put(
        "artifact.paired_reads_in_flight",
        count(MAX_ARTIFACT_READS_IN_FLIGHT),
    );
    put("artifact.host_open_publishers", count(MAX_OPEN_PUBLISHERS));
    put(
        "artifact.environment_files",
        u64::from(ArtifactBudget::LEASE.files),
    );
    put("artifact.environment_bytes", ArtifactBudget::LEASE.bytes);
    put(
        "artifact.conversation_records",
        count(ArtifactRecord::MAX_PER_CONVERSATION),
    );
    put("artifact.read_resumes", u64::from(ARTIFACT_READ_RESUMES));
    put(
        "artifact.read_range_bytes",
        count(HeldReads::MAX_RANGE_BYTES),
    );
    put("artifact.read_busy_wait", millis(HeldReads::BUSY_WAIT));
    put("server.artifact_reads", count(HeldReads::CONCURRENT_READS));
    put(
        "artifact.publish_deadline",
        millis(AttachmentLimits::default().publish_deadline),
    );
    put("artifact.publish_call_wait", millis(PUBLISH_CALL_WAIT));
    put("artifact.fetch_bytes", ArtifactBudget::LEASE.fetch_bytes);
    put("artifact.fetch_call_wait", millis(FETCH_CALL_WAIT));
    put("server.fetch_reads", count(FETCH_READS));
    put(
        "artifact.fetch_busy_backoff",
        millis(FETCH_BUSY_BACKOFF[FETCH_BUSY_BACKOFF.len() - 1]),
    );
    put(
        "artifact.host_fetches_in_flight",
        count(MAX_FETCHES_IN_FLIGHT),
    );
    put("peer.poll_interval", millis(POLL_INTERVAL));
    put("peer.poll_backoff_cap", millis(POLL_BACKOFF_CAP));
    put("peer.read_budget", millis(READ_BUDGET));
    put("peer.reads_at_once", count(READS_AT_ONCE));
    put("peer.preempt", millis(PREEMPT));
    put("peer.audit_queue", count(AUDIT_QUEUE));
    put("peer.withdrawn_per_read", count(WITHDRAWN_PER_READ));
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
         The client's foreground phone sends are named here and counted only in \
         `FOREGROUND_SENDS` (`crates/nessa-client-core/src/outbox/domain/decide.rs`); \
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
         how many times a preparing read is asked again |\n\
         | client.foreground_sends | client | the client's own | \
         `FOREGROUND_SENDS` in `crates/nessa-client-core/src/outbox/domain/decide.rs` | \
         how many times one foreground phone send is asked |\n",
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
        let foreground = rendered
            .lines()
            .find(|line| line.contains("client.foreground_sends"))
            .expect("foreground row");
        assert!(foreground.contains("FOREGROUND_SENDS"));
        assert!(foreground.contains("the client's own"));
        assert!(!foreground.contains("| 3 |"));
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
