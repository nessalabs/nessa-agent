# Tool observations and review input

A `ToolCall` entity owns one tool identity within an execution. It never executes
or authorizes a tool. `ToolCallUpdate` is immutable sparse input; `ToolObservation`
is the immutable accumulated snapshot without identity. The entity validates
execution/tool correlation and replaces its observation with a consuming merge.
Unchanged payload allocations are retained; replacements move into the new state.

```text
provider patch --> ToolCallUpdate --> ToolCall --> ToolObservation
                                        |              |
                                  identity checks      +--> permission review
```

Arrows show accepted input and projected state. `None` in an update leaves the
old field unchanged; an explicit empty collection clears it. Captured review
snapshots do not change when later tool updates arrive.

The worker keeps RPC IDs and process effects outside the domain and calls an
application `ExecutionController` for observation merging, permission coordination,
and event projections. The controller retains each permission's exact review input
and constructs its attributed resolution; callers cannot combine an answered
request with arbitrary review arguments. Resolved and cancelled identities remain
reserved until the execution finishes. Finishing also releases tool observations;
sparse status updates retain unchanged content allocations. Permission events
include the tool state merged so far and the exact configured options. Normal
tool events remain sparse. Paths are untrusted descriptions, never filesystem
access or authorization; wire size bounds remain adapter rules.

`ToolContent::text` and `ToolContent::diff` own compact, immutable observed text.
Use `view()` to inspect `ToolContentView` through shared references. Empty text is
valid; a diff's missing prior text (`None`) remains distinct from explicitly empty
prior text. `payload_bytes()` measures retained text and path allocation; the owning
tool observation separately counts collection slots. These values describe provider
output and do not apply changes or authorize filesystem access.

`ToolContent::structured` keeps a tool's structured result (MCP
`structuredContent`) as the JSON text an adapter wrote, at most
`MAX_STRUCTURED_RESULT_BYTES`; it sits beside the result's text, never instead of
it. `McpTool` names the MCP server and tool a call went to, bounded by
`MAX_MCP_NAME_BYTES`; `ToolCallUpdate::with_mcp_tool` attaches it, and a later
update that names none keeps the observed one. `ToolCallUpdate::with_content`
replaces an update's content collection, as an adapter does when a harness
reports a result outside ACP content. The names are the harness's spelling:
Claude's replaces characters outside `[A-Za-z0-9_-]` in a tool name with `_`. It is identity for display and
correlation, not access to the server. Which harness can say what is recorded in
[ADR 344](../../../../docs/adr/todo/344-mcp-ui.md#what-each-harness-passes-through-acp):
Claude's name is split at the one configured server prefix it starts with, Codex's
comes exactly from `rawInput`, and Opencode's cannot be split, so it names none.
No harness passes a tool's own `_meta` (its MCP Apps UI resource) through ACP.

Tool schemas and raw arguments are parsed by infrastructure. The application's
`tools::ToolReviewInput` preserves complete review arguments as opaque JSON text; it is
not a domain entity or a grant. [Permission review](permissions.md) pairs this
input with the exact request. [Transport limits](transport.md) bound retained
payloads separately from frames and queued events.

Claude's profile now accepts bounded native tool names and configured MCP server
names, retaining original permission inputs. Web fetch, execution and thinking
categories retain their distinct domain/storage values. Native Bash and mode
changes remain disabled; Nessa's shell is provided through MCP. Trusted
`AcpConfig::mcp_servers` configurations are not part of restoration identity
(ADR 344): the server list is a per-open attachment, so adding, editing or
removing an MCP server no longer changes a saved session's identity. A host that
relays the servers may refuse one changed while it runs; the gateway reads its
list once per run, and its relay's `configuration-changed` refusal guards the
live settings that #480 introduces.
