# 0009. Separate agent harnesses from optional Nessa tools

- **Date:** 2026-09-04
- **Status:** proposed
- **Related:** [0007](0007-nessa-session-protocol-and-authorities.md) (product wire,
  identity, authorization), [0008](0008-reusable-event-stream-crate.md) (streams)
- **Integration details:** [MCP sequences and tools](../../design/collaboration-sequences-and-mcp.md)

## Context

Nessa should let Claude, Codex, and other agents interact with Nessa threads,
surfaces, and collaborators without becoming a replacement for their harness.
Users must be able to keep, disable, replace, or remove Nessa tools
independently of their agent. Nessa will also have its own internal
agent/harness, initially using the same optional product tools as other agents.
Tool packages should be developable and releasable outside Nessa. This is a
dependency and extension boundary, separate from ADR 0007's wire design.

## Decision

**Provider harnesses remain externally owned and unmodified.** Claude runs in
its Claude harness, Codex in its Codex harness. Their execution loop, built-in
tools, context management, configuration, model credentials, and permission
policy remain theirs. Nessa does not patch/fork their runtime, intercept
internal tool dispatch, rewrite their system instructions, or recreate those
harnesses around a model API. A provider SDK qualifies only if it preserves that
harness; a model/Agents SDK is not automatically equivalent to the corresponding
coding agent product.

**Nessa has its own agent without replacing external harnesses.** The internal
agent's execution loop belongs to Nessa and may evolve independently. Initially
it consumes the same MCP or CLI tools and scoped grants as external agents; it
gets no implicit owner token, direct database access, or private handler
shortcut. The rule against modifying a provider harness does not prohibit
Nessa's own harness.

**One product operation catalog, two optional tool interfaces.** Expose Nessa
capabilities through MCP tools and a shell-friendly CLI, both backed by the same
public `NessaClient` methods and generated schemas. Agents may use either
according to their harness and user preference. MCP provides structured
discovery and calls; the CLI provides commands with structured JSON results and
exit codes for agents that already run shell tools. Neither is the product
implementation.

Use these interfaces to list/create/show threads, read authorized history,
message sessions, and control Nessa-owned resources within an explicit grant.
Preserve existing built-in provider tools: “Nessa capabilities are tools” does
not mean all harness behavior must be rewritten as Nessa tools. There is no
hidden runtime injection or requirement to install MCP when the configured CLI
is sufficient.

**Separate the two directions.** A gateway binding may start/attach to a
harness, send user input, observe output, and relay supported lifecycle/approval
operations through its supported external interface (e.g. ACP). That is host
integration. Agent-initiated Nessa product operations take this path:

```text
External harness or Nessa internal agent
    → optional Nessa MCP tools OR Nessa CLI commands
    → scoped NessaClient instance
    → Nessa Session Protocol
    → gateway authorization and product handlers
```

The binding is not an agent implementation. Unsupported host controls remain
unavailable; optional MCP cannot imply access to a harness's full transcript or
internal state. Nessa owns its conversations, routing, and event history; it
does not override harness policy. A Nessa token grants gateway access, not
permission to bypass the provider's sandbox or tool approvals.

**Make extension packages independent and removable.** Start with a separately
packaged `NessaMCP` and CLI using public `NessaClient` APIs and configurable
tool groups (discovery/read, collaboration, creation/control). Add separately
installable MCP servers when distinct capabilities justify separate dependencies
or releases; do not require a server per tool. No gateway internals, provider
runtime imports, or Nessa UI dependency in these packages. Third parties can
implement their own MCP/CLI packages against the same versioned client and
scoped gateway contract.

**Users choose the installed and exposed capabilities.** The configured tool
allowlist, current gateway grant, implemented capability, and harness policy all
constrain availability. Check the server allowlist on calls as well as
discovery; a stale cached tool cannot bypass removal. Use normal MCP or
shell-tool configuration and preserve unrelated tools/settings. Removing MCP
removes that interface; disabling the capability across both paths requires
narrowing/revoking its gateway grant. Never auto-fallback from a denied MCP call
to CLI to evade a tool policy. Each execution path remains subject to the
harness policy and an explicitly configured credential/profile. Nessa-created
sessions use the user's selected integration profile; creating a session is not
consent to enable every tool. An external session is configured explicitly by
its user. No auto-reinstallation or forced enablement after removal, restart, or
upgrade.

Tool installation/configuration, scoped credential issuance, and
provider/session creation are separate operations. Removing an interface
disables its tools without breaking the harness's ordinary work or native Nessa
surfaces; another interface works only if separately enabled. Revocation blocks
future authorized calls; removal does not roll back committed messages or
completed effects. In-flight accepted work follows the product's explicit
cancellation policy.

## Alternatives considered

- **Patch providers or build a unified replacement harness:** couples Nessa to
  provider internals and takes over behavior the user wants to preserve.
- **Inject Nessa tools directly into each provider runtime:** creates a second
  extension path users cannot manage through their chosen tool interface.
- **Require all Nessa tools for every session:** prevents minimal/read-only setups
  and makes optional collaboration a dependency of ordinary agent execution.
- **Implement MCP and CLI behavior separately:** duplicates authorization, retries,
  and lifecycle rules; both call the same client contract.
- **Bundle tool implementation into gateway internals:** prevents independent
  releases and bypasses the shared public client contract.

## Consequences

Agents retain their existing harness behavior. Users can remove Nessa
integration and keep using the agent. MCP and CLI packages can evolve
independently while all gateway access still passes through scoped `NessaClient`
instances.

We must test supported harness/MCP versions, configuration isolation, selected
tool profiles, removal, and capability loss. Some providers may not expose all
desired host controls; report that limitation instead of modifying their
internals. ADR 0007 owns commands and authorization; this record owns harness
preservation and the optional MCP/CLI extension model, and the distinct Nessa
internal harness. Package location/name and release versioning are
implementation choices; no agent configurations change here.