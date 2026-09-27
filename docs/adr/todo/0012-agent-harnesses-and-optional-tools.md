# 0012. Separate agent harnesses from optional Nessa tools

## Purpose

Let agents use working Nessa operations as optional tools. The tools call the
existing product API; they do not add another system for running agents, checking
permissions, or retrying work. The first Nessa conversation must work before any
optional tools are installed.

- **Date:** 2026-09-04; revised 2026-09-07
- **Status:** proposed — optional tool packages are not implemented
- **Related:** [0008 — runtime/bindings](0008-agent-client-api.md),
  [0011 — shared operations](0011-nessa-session-protocol-and-authorities.md),
  [0010 — authentication](../done/0010-local-authentication.md)
- **Integration examples:** [Client/MCP sequences](../../design/collaboration-sequences-and-mcp.md)

## Two directions, different responsibilities

A **harness** is the program that runs an agent's model and tools. A **binding**
is Nessa's adapter to that program. An **MCP adapter** lets that agent call Nessa
product tools through the Model Context Protocol. These calls travel in two
directions:

```text
NessaClient → gateway → nessa-sdk → ACP binding → external agent
                          ▲                         │
                          │                         │ optional Nessa tool call
                          └─ gateway ← NessaClient ← MCP adapter
```

In the first direction, Nessa asks the agent to work and controls its turn.
ADR 0008 owns the binding, turn state, task supervision, and Nessa records.
In the second direction, the agent asks Nessa to do something, such as read a
conversation. The optional tool translates that request into an ordinary
permission-checked API call. A future CLI can make the same calls without MCP.

The external harness keeps its model/tool loop, built-in tools, context, model
credentials, and permission checks. Nessa does not patch that loop, intercept
private tool calls, rewrite system instructions, or replace the harness with a
model API call. Nessa checks access to its operations; the provider checks access
to its own operations. Neither credential bypasses the other system's checks.
Controls that the harness cannot support remain unavailable.

A future Nessa-owned agent may have its own loop, but this ADR does not build it
or wait for it. Such an agent would call the same public APIs with its own limited
permissions. It would not get an owner token, write directly to the database, or
skip the API by calling private command handlers.

## One operation implementation

The SDK behind the gateway owns product commands, state changes, and receipts
(the saved responses proving which commands were accepted). `NessaClient` owns
typed calls, connections, wire validation, and matching replies to requests.
The MCP/CLI adapter has four jobs: validate its arguments, check its enabled tool
list, call NessaClient, and format the result.

Use the existing generated schemas for product arguments, results, and errors.
A tool can have a convenient name, but it must call an existing operation with
tested resource checks. It cannot introduce new behavior, duplicate turn rules,
or pass arbitrary provider methods and options through the API.

Keep the product `requestId` unchanged when retrying the same command after an
uncertain result. A transport request ID identifies one network attempt; it does
not replace the product's retry ID. Preserve typed errors. Do not add a tool-owned
queue, receipt store, automatic prompt replay, busy-turn retry loop, or cursor
allocator. A receipt proves acceptance. It does not prove the turn finished, the
model used a message, or a window displayed it.

For example, if a tool sends a message and loses the reply, it retries with the
same `requestId`. Nessa returns the original receipt. Generating another ID could
record the same message twice.

MCP cancellation, timeout, closed stdio, and CLI exit stop waiting or observing.
They do not undo a saved command or cancel a Nessa turn. Use the explicit
`turn.cancel` operation when allowed. If the connection closes while Nessa may
be accepting a command, check the result using its original `requestId`.

`turn.cancel` follows [ADR 0008's cleanup contract](0008-agent-client-api.md#interruption-and-resource-cleanup).
The tool must make clear whether Nessa accepted Stop or has confirmed that work
and cleanup finished. Closing an MCP request or killing a CLI does not prove the
target stopped. The SDK/binding owns that cleanup. Stopping a turn also releases
adapter processes and requests that belong only to it. A shared profile adapter
keeps its host-managed lifetime. None of these actions undoes a message or turn
already accepted independently by another conversation.

An agent may call a Nessa tool while its own turn is running. The coordinator
must still handle reads, approvals, Stop, and allowed inbox requests. A tool must
not wait for a new turn on the same busy conversation, or wait for an outcome that
requires the tool itself to return. Otherwise the agent waits for the tool while
the tool waits for the agent, and neither can finish. Return a receipt, a read
result with size/time limits, or `turn_busy`. Never hold the lock for accepting
commands while waiting for a tool or network response.

## Minimal package and credential scope

Start with one separately packaged local MCP adapter using **stdio**: the host
communicates with the adapter process through standard input and output. Give it
its own NessaClient. It needs no gateway HTTP MCP endpoint, OAuth exchange service,
plugin registry, copied event store, or provider-specific execution framework.
Use the chosen MCP SDK's connection and cleanup support. Test its protocol version
and features with the actual target host before claiming support.

Startup code creates one client for each attached principal/profile. A principal
is the authenticated caller; a profile chooses the tools exposed to it. Do not
borrow the panel's client, change a shared client's token, or fetch credentials
from a global service locator. Closing the adapter closes its own subscriptions
and connection. Accepted conversation work remains owned by the runtime.

Use ADR 0010's protected local credential sources and explicit provisioning.
Keep secrets out of tool arguments, results, prompts, URLs, and command-line flags.
The adapter cannot grant itself more access or trust caller-supplied author fields.
A call must pass both checks: the profile enables the tool and the gateway permits
its effect on the target resource.

The first profile reads allowed conversation metadata and history in limited
pages. Add create/start/cancel after those runtime operations pass acceptance
checks. Add message/status after ADR 0011 phase B. Approval responses need explicit
opt-in and their own permission. A large tool catalog and remote window navigation
can wait; they are not needed for the first useful package.

## Optional installation and removal

Users explicitly choose the tools/profile for a session. Installing tools,
issuing credentials, and starting the provider are separate operations. Ordinary
Claude execution through ACP works with no Nessa MCP package installed.

Check the enabled tool list both when listing tools and when calling one. A host
may remember a tool after removal; calling it must fail locally. The gateway also
checks current access. Removal stops new calls through that interface. It does
not undo accepted work, revoke another profile, or disable other enabled
interfaces. To withdraw the permission across interfaces, revoke it at the gateway
under ADR 0010's rules: previously allowed work may finish, later checks deny it.
Only the runtime owns explicit turn cancellation.

Keep unrelated harness tools, settings, and model configuration unchanged.
Refreshing a tool list must not restart an active harness without an explicit
operation. Do not enable tools automatically after restart/upgrade or retry a
denied MCP action through a CLI. If removal requires a host reload, say so; an
outdated tool list still grants no permission.

## Delivery and completion

1. Once ADR 0011 phase A's read APIs work, deliver the stdio read profile. Test
   installation/removal with one real MCP host and a pinned SDK/protocol profile.
   Native conversations must continue to work without the adapter.
2. Add selected control and collaboration tools as their product operations land.
   Check that direct wire calls and MCP calls preserve the same receipts, errors,
   author information, and current permissions.
3. Add an automation CLI only when a real caller needs it. It calls NessaClient
   directly, emits structured output, and keeps `--request-id` for explicit
   retries. MCP and CLI do not need identical feature sets before either ships.

Verify two adapters with separate permissions and clients; access changes after
listing tools; lost receipts; cancellation/closure after a command is saved;
calls to removed tools; read limits; calls back into a busy runtime; and preserving
configuration during installation/removal. Reuse the SDK's turn tests and the
gateway's policy tests. Each package tests its own mapping and isolation instead
of rebuilding those rules or repeating their entire test suites. An optional tool
failure must not take down the gateway or another agent.

Keep this ADR proposed/in `todo/` until there is test evidence for the first
package and any selected tool groups. Later CLI support, HTTP MCP hosting,
automatic installation, provider-session import, remote pairing, and Nessa's own
harness remain deferred; they do not prevent the first package from finishing.

## Consequences

The runtime can ship before optional tools, and tools can ship before peer
messaging. Small adapters can be developed independently against the public client.
Each new interface reuses working operations while the SDK keeps ownership of
agent execution and conversation state.
