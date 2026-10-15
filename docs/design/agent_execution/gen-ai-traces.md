# GenAI traces

A turn on the Claude, Codex, or OpenCode path produces spans that follow the
OpenTelemetry GenAI semantic conventions. The spans are derived from the
session updates and permission events the ACP worker already decodes. The
recorder does not call the harness and does not open a network connection.

`LocalTraceStore` keeps the finished spans. `Agent::read_traces` returns the
ones for a conversation, in the order they closed. A turn still running is
absent until its agent span closes. The store keeps the most recent 2048
finished spans, and the same number of child spans from turns that are still
open. Those children are dropped one span at a time, oldest first. Captured
message and tool text is capped at 8 MiB across finished and open turns.
Older finished spans are dropped while more than one remains; the last
finished span stays, and open-turn children are dropped after that. A span
whose own captured text exceeds that is stored without the oversized
attribute.

`SpanExporter` is the plug for a later user-chosen OTLP endpoint.
`LocalTraceStore::set_exporter` installs it. No endpoint is read now, and the
store must not be installed as its own exporter.

Message text, tool arguments, and tool results stay off a span unless the
store is built with content capture on. Thought text is never stored.

## What a turn contains

| What the harness does | Spans |
| --- | --- |
| Prompt is dispatched | `invoke_agent {agent}` and the first `chat {model}` open |
| Text or thought arrives | Appended to the open chat span. Thought text is not stored |
| A tool call is announced | The open chat span closes with finish reason `tool_calls`. `execute_tool {name}` opens as a child of the agent span |
| The tool completes or fails | The tool span closes. The next chat span opens when the harness speaks again and no tool is still open |
| The turn finishes | The open chat span, any open tool spans, and the agent span close with the turn's finish reason |
| `usage_update` before a turn | Held and applied when the turn starts |
| `usage_update` during a turn | Context figures go on the agent span. Input and output tokens, when the payload actually has them, go on the open chat span and are summed onto the agent span |
| A permission request, answer, cancel, decline, or deadline | `nessa.permission.*` on the tool span. A request that arrives before the tool update opens the tool span. A decline that names a tool is attached only when one open tool has that name; otherwise it is recorded on the agent span |
| The recorder is dropped with a turn open | The turn closes as `cancelled` |

A turn that only speaks is an agent span and one chat span. A turn that calls
a tool is an agent span, a chat span, the tool span, and a later chat span
only when the harness speaks again. Two tools in flight do not open a chat
span between them.

## Standard attributes

`gen_ai.operation.name` is `invoke_agent`, `chat`, or `execute_tool`.
`gen_ai.provider.name` is the model provider (`anthropic`, `openai`,
`opencode`). `gen_ai.agent.name` is the harness (`claude`, `codex`) or
`agent` when the binding has no harness name. `gen_ai.conversation.id` is the
conversation the session was opened for. `gen_ai.request.model` is the model
the binding selected. `gen_ai.output.type` is `text`.

`gen_ai.response.finish_reasons` is one of `stop`, `length`,
`max_turn_requests`, `refusal`, `cancelled`, or `tool_calls`. `stop` and
`tool_calls` are successful. The others set `error.type` to `output_limit`,
`request_limit`, `refused`, or `cancelled`. A failed tool sets `error.type`
to `tool_failed`.

`gen_ai.usage.input_tokens` and `gen_ai.usage.output_tokens` are set only when
the harness reports `inputTokens` or `outputTokens` (or the same names with
underscores). ACP `usage_update.used` is context occupancy, not input tokens.
`gen_ai.usage.reasoning.output_tokens` and
`gen_ai.usage.cache_read.input_tokens` follow the same rule for
`thoughtTokens` and `cachedReadTokens`.

`gen_ai.tool.name` is the provider's tool name when the profile kept one
(Claude's `_meta.claudeCode.toolName` or ACP `name`; Codex's `name`, otherwise
its kind). A kind label stored first (`read`, `edit`, `search`, `fetch`,
`execute`, `think`, `delete`, `move`, `switch_mode`, `other`) is replaced when
a later update carries the provider's tool name. A specific name is left in
place when a later update only repeats the kind. `gen_ai.tool.call.id` is the
tool-call id. `gen_ai.tool.type` is `function`, or `extension` when the call
names an MCP tool.

When content capture is on, `gen_ai.input.messages`, `gen_ai.output.messages`,
`gen_ai.tool.call.arguments`, and `gen_ai.tool.call.result` carry the text.
They are absent otherwise.

## `nessa.*` attributes

These are the facts the GenAI conventions do not name. A span carries one
only when that fact was observed.

| Attribute | Where | Meaning |
| --- | --- | --- |
| `nessa.execution.id` | Every span of the turn | The turn's execution id |
| `nessa.hook.native` | Agent span | Whether this launch allowed the harness's own hooks. Absent when the binding has no such choice |
| `nessa.hook.event` | Agent span | The hook that ran. A permission-review hook is `before_permission_review` |
| `nessa.hook.verdict` | Agent span | `allow`, `refuse`, `stopped`, `error`, `panic`, `timeout`, or `busy` |
| `nessa.hook.count` | Agent span | How many hook notes were applied to the turn. The event and verdict are the latest |
| `nessa.permission.id` | Tool span, or the agent span when the decline names no open tool or more than one open tool has that name | The review id |
| `nessa.permission.effect` | Tool span | `allow` or `deny`, once a decision exists |
| `nessa.permission.state` | Tool span | `requested`, `answered`, `cancelled`, `declined`, or `deadline` |
| `nessa.permission.decider` | Tool span | `person` for an explicit choice, `rule` for a remembered rule, `mode` for the host's approval mode, `hook` for a permission-review hook, `deadline` when the deadline answered, or `provider` or `runtime` when that side withdrew the review |
| `nessa.grant.id` | Agent span | The share grant the turn was started under |
| `nessa.grant.policy_revision` | Agent span | The revision of that grant the host checked |
| `nessa.grant.tools_checked` | Agent span | Whether the host checked the turn's tool calls against the grant |
| `nessa.subagent.id` | Agent span | A child agent this turn spawned. A later note replaces the pair |
| `nessa.subagent.parent` | Agent span | The parent of that child |
| `nessa.context.used_tokens` | Agent span | ACP `usage_update.used`: tokens occupying the context window. Not model input |
| `nessa.context.size_tokens` | Agent span | ACP `usage_update.size`: the context window |
| `nessa.context.cost.amount` | Agent span | The cost amount the harness reported, as it spelled the number |
| `nessa.context.cost.currency` | Agent span | The currency that cost was denominated in |

A grant or hook note can arrive before the worker exports the agent span. The
store keeps the note, keyed by conversation and execution, and applies it when
the span closes. A note that arrives after the span is stored replaces that
span's attributes. Unconsumed notes are capped at 2048; the oldest is dropped.
An invocation that ends without a span consumes its own note.

Child spans stay out of `read` until the agent span closes. Two conversations
that reuse an execution id do not share a trace or a note.
