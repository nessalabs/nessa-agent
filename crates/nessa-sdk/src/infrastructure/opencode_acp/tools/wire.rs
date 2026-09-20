use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::tools::ToolReviewInput;
use crate::domain::agent_execution::tools::ToolCallUpdate;
use crate::infrastructure::acp::fields::identifier;
use crate::infrastructure::acp::tools::wire::tool_call as acp_tool_call;
use crate::infrastructure::json_rpc::protocol;
use serde_json::Value;
use std::collections::HashMap;

/// The most tool calls one execution keeps an identity for.
///
/// The same bound the other profiles use. With 256-byte identifiers and the
/// protocol's ten kinds — the only two things this map holds — it bounds the
/// payload independently of how large the frames arriving are, so a provider
/// that keeps announcing tool calls cannot decide how much memory the adapter
/// spends.
const MAX_TOOLS: usize = 4096;

/// What one observed Opencode tool call is known by.
///
/// The ACP kind, and deliberately not the title.
///
/// A title is display text. Opencode composes it for a person to read, and it
/// can be derived from arguments the model supplied, so it is neither bounded
/// nor guaranteed to be on one line, and nothing makes it agree with `rawInput`
/// — "Read config" is a legal title on a request to edit something. It is also
/// what `ToolReviewInput::name` becomes, which is the label a person approves
/// under and the identity the audit record keeps. An approval is worth what its
/// record is worth, so the name has to be something the provider cannot choose
/// the meaning of.
///
/// The kind is: the protocol defines ten of them, the shared mapper refuses
/// anything else before this profile sees it, and it says what class of action
/// is being asked for. What the action is *on* is in `rawInput`, which
/// [`permission_input`] requires. Between the two, a review has a trustworthy
/// identity and the whole of the arguments, which is what the title was being
/// asked to stand in for.
///
/// Retained rather than read from the request, because a permission request can
/// arrive naming nothing but the tool call it belongs to.
///
/// [`permission_input`] read the title for one case and no longer does. Why
/// that case cannot be rescued is documented there, and it is the rule above
/// holding rather than bending.
#[derive(Clone)]
pub(in crate::infrastructure::opencode_acp) struct ObservedTool {
    kind: Option<String>,
}

/// The ACP kind that says nothing.
///
/// One of the protocol's ten, so the shared mapper accepts it, and the one
/// Opencode maps everything it has no case for to. See [`permission_input`].
const UNINFORMATIVE_KIND: &str = "other";

/// The ACP kind on this frame, as a string, once the shared mapper has accepted
/// it. Safe to keep only in that order: `acp_tool_call` has already refused
/// anything outside the protocol's ten, so what is read back here is one of
/// them and not arbitrary provider text.
fn accepted_kind(value: &Value) -> Option<String> {
    value.get("kind").and_then(Value::as_str).map(str::to_owned)
}

/// Read a tool call frame, remembering what it was.
///
/// The reading itself is the shared one: Opencode is assumed to report tool
/// calls in the protocol's own shape, with no extension of its own to
/// interpret. Assumed, not observed — a tool call only happens during a model
/// turn, and none could be run here. What is added is the retention, which is
/// what makes a later permission request answerable.
pub(in crate::infrastructure::opencode_acp) fn tool_call(
    value: &Value,
    tools: &mut HashMap<String, ObservedTool>,
) -> Result<ToolCallUpdate, AgentError> {
    let update = acp_tool_call(value)?;
    let id = identifier(value, "toolCallId")?;
    let kind = accepted_kind(value);
    // A tool call is announced once and then updated, and an update carries only
    // what changed. A kind that is absent now is one the announcement already
    // gave, so it is kept rather than forgotten.
    if let Some(observed) = tools.get_mut(id) {
        if kind.is_some() {
            observed.kind = kind;
        }
        return Ok(update);
    }
    // Bounded, and by refusing rather than by evicting: a review that named the
    // wrong tool call would be worse than one saying it cannot name it, and this
    // many tool calls in one execution is not a real one.
    if tools.len() >= MAX_TOOLS {
        return Err(protocol("too many tool calls in one execution"));
    }
    tools.insert(id.to_owned(), ObservedTool { kind });
    Ok(update)
}

/// What a permission request is asking to be allowed.
///
/// Opencode is assumed to ask with the tool call itself, so its arguments are
/// read from `rawInput` — assumed, for the reason [`tool_call`] gives. A request
/// carrying no arguments is refused rather than reviewed on a label alone: a
/// decision recorded against "Edit" with nothing saying what would be edited is
/// not a reviewed decision, and the audit would carry it as though it were.
///
/// The name is the request's own kind, or the one the announcement gave. A
/// request whose kind is `other`, or which names no kind at all, is refused
/// rather than reviewed under something that is not an identity.
///
/// `other` is what Opencode maps every permission category it has no case for
/// to: `skill`, `lsp`, `todowrite`, `question`, `websearch`,
/// `external_directory`, and every MCP tool, which is to say Nessa's own
/// shell. An approval recorded as `other` says a thing was approved and
/// nothing about which thing, which is the same record as one recorded under
/// `tc_01H9` — so it is refused for the reason that one always was.
///
/// The title cannot stand in, and the reason is worth keeping because it is
/// not obvious. On a *permission* request the title is not the announcement's:
/// upstream builds the frame with `state.title = permissionTitle(toolName,
/// metadata)`, and the ACP title is then that value or, when it is absent, the
/// permission key. For most of the `other` set `permissionTitle` has no case
/// and returns nothing, so the title is the key — `nessa_shell`, `skill` —
/// which would be a real identity. For two of them it has a case and builds
/// the title out of the model's own arguments: `websearch` from the query,
/// `external_directory` from the description, command or target path. Nothing
/// on the frame says which of the two happened. A search for `read` is one
/// short graphic-ASCII word, so any bound a name can carry admits it, and the
/// record would read `read` for a web search — a name the model chose the
/// meaning of, which is worse than one that means nothing.
///
/// What would rescue it is the permission's category, which upstream knows and
/// ACP does not carry. Until something does, `other` has no reviewable
/// identity and saying so is the honest answer.
pub(in crate::infrastructure::opencode_acp) fn permission_input(
    request: &Value,
    tools: &HashMap<String, ObservedTool>,
) -> Result<ToolReviewInput, AgentError> {
    let tool = request
        .get("toolCall")
        .ok_or_else(|| protocol("missing permission tool"))?;
    let id = identifier(tool, "toolCallId")?;
    let arguments = tool
        .get("rawInput")
        .filter(|arguments| arguments.is_object())
        .ok_or_else(|| protocol("permission request carries no reviewable input"))?;
    // Validated by the same rule as a kind arriving on a tool call frame, and
    // by the same code: a kind is not more trustworthy for having arrived on a
    // permission request. `acp_tool_call` refuses anything outside the
    // protocol's ten, which is what makes reading the string back safe.
    acp_tool_call(tool)?;
    let name = accepted_kind(tool)
        .or_else(|| tools.get(id).and_then(|observed| observed.kind.clone()))
        .filter(|kind| kind != UNINFORMATIVE_KIND)
        .ok_or_else(|| protocol("permission request names no reviewable tool"))?;
    Ok(ToolReviewInput {
        name,
        arguments_json: arguments.to_string(),
    })
}

#[cfg(test)]
#[path = "../../../../tests/infrastructure/opencode_acp/tools/wire.rs"]
mod tests;
