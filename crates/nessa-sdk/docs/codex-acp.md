# Local Codex ACP binding

This provider guide covers the local Codex harness. Start with the
[agent execution guide](agent_execution/README.md) for domain ownership, lifecycle,
permissions, prompts, and shared transport contracts, and read the
[Claude guide](claude-acp.md) alongside it: everything the three bindings share —
process supervision, restoration identity, queueing and steering, the hook
contract, the credential and context environment split — is stated there and is
not repeated here. What follows is what is Codex's own. One shared thing this
binding opts out of: Codex steers by queue, because its adapter advertises the
steering extension without the outcome contract the shared runtime requires —
see `codex_acp/sessions/profile.rs`.

## Composition and application boundary

Identical to the Claude binding in shape. The host supplies a selected
`ModelMetadata`, explicit `TokenLimits`, an `infrastructure::acp::sessions::AcpConfig`,
and an `Arc<dyn ExecutionAudit>` to `CodexAcpProvider::new`. The factory is
immutable, and `Agent::prepare(provider, manager, audit)` loads durable evidence; `authorize_attachment` and `start_attachment` open or resume the context.

```text
host -> Agent(provider, SessionManager)
          |-> CodexAcpProvider -> ProviderSession -> ACP worker
          |-> SessionManager -> SessionStorageLease
          |-> hooks + optional UI subscribers
```

The worker, the transport, the correlation, the deadlines and the cleanup are the
same code the Claude binding runs on. What differs is behind `AcpProfile`:

- **Session configuration is an ordered list.** Codex takes its model and its
  approval preset through separate `session/set_config_option` requests and the
  model must be settled first, so `AcpProfile::session_configuration` returns the
  steps in the order the provider must receive them. Only the last response is
  verified as fully configured — it must read back both selections exactly, and
  nothing is published as ready before it does. An earlier response is checked
  only for the model being one Codex offers, because a selection this binding has
  not made yet cannot be required to read back. The same allowance covers a
  `config_option_update` notification arriving mid-configuration, which is Codex
  reporting the state it still has. Duplicate configuration IDs are rejected
  before any value is read.
- **A permission request is passed whole.** Codex puts the reviewable facts in
  `_meta.codex` and does not always send `toolCall.rawInput`, so
  `AcpProfile::permission_input` receives the complete request rather than its
  `toolCall`. The binding prefers `rawInput` and falls back to `_meta.codex`;
  neither present is a request that fails closed rather than one reviewed on
  nothing.

## Supported native profile

- Unix process groups; Windows configuration is rejected before starting a child.
- Exact OpenAI model selected from the catalog, set through `CODEX_CONFIG` at
  launch and then through the session's `model` option, and read back over the
  protocol. A model the provider does not offer, or reports having selected
  something else for, closes the binding rather than running whatever it chose.
- **`read-only` is the only preset this binding opens a session in.** It is set
  twice on purpose: in `INITIAL_AGENT_MODE` so the session is never briefly open
  in a more permissive one, and then explicitly, so the mode the provider
  reports is the mode that was asked for.
- What that preset does and does not buy is worth stating plainly, because it is
  the largest honest difference from the Claude binding. Codex runs its own shell
  and patch tools inside a sandbox of its own and offers **no way for a client to
  remove them**. Anything leaving that sandbox — a command outside the workspace,
  a file outside it, the network — is asked for and answered through Nessa's
  permission owner. Work Codex does *inside* the sandbox is observed and audited,
  not reviewed beforehand. There is no Codex mode that asks for everything, so a
  binding configured with tools disabled is refused rather than accepted as a
  text-only Codex that does not exist.
- Image input uses the shared ACP byte-source and prompt transport. The factory offers
  it only with an injected `AcpConfig.images` source and model image limits; the
  negotiated harness must also advertise `promptCapabilities.image`. The checked
  Terra catalog profile admits PNG/JPEG up to 5,000,000 encoded bytes and 2048 pixels
  per edge, Nessa delivery ceilings rather than the provider maximum. Other OpenAI
  catalog models remain without image admission limits. The capability combinations
  and exact content blocks are covered by `codex_images_follow_the_model_source_and_negotiated_offer`.
- Permission scopes are exact-request only. A reusable session or application
  scope is refused at construction: this profile cannot enforce one, and
  accepting it would be a promise about later requests that nothing keeps.
- Codex sends content this client does not advertise. Nessa declares
  `terminal: false`, and the shared mapper rejects `{"type":"terminal"}` and
  `resource_link` content; the profile normalizes both into the shared
  vocabulary before the mapper sees them — a terminal pointer is dropped, a
  resource link becomes text naming its URI, and streamed terminal output is
  carried as text. The completion's aggregate `rawOutput.formatted_output` is
  carried only when nothing was streamed, so command output is never recorded
  twice.
- Binding ceilings of **200,000 context tokens and 64,000 output tokens**,
  narrowed further by model facts and the host's explicit limits. These bound
  what this binding admits, not what the provider does: Codex owns its own
  context window and compacts it without telling Nessa, and takes no per-turn
  output limit through ACP. Unlike the Claude binding, there is no harness
  variable to hand the reservation to — it is Nessa's admission budget only, and
  this guide does not claim otherwise.
- Nessa's own instructions reach Codex through `CODEX_CONFIG`'s `instructions`,
  not through the ACP session request, because that is where Codex reads them.
- **User-configured hooks and the legacy notify command are requested off, and
  that request is not treated as proof.** `CODEX_CONFIG` is JSON the pinned
  adapter forwards as the `thread/start` config override: `features.hooks` is
  `false` and `notify` is `[]`, on every launch including one that also carries
  instructions. On the pinned loader that override sits above ordinary user and
  project layers and below legacy managed configuration and MDM, so those later
  layers can turn either setting back on. ACP exposes no effective hooks or
  notify value and discards hook events, so their absence is not attestation.
  Provider builtin cleanup is a separate lifecycle path and is not disabled to
  make the request look complete. A negotiated session therefore reports native
  hook suppression as unsupported. The binding does not claim Nessa owns Codex's
  hook boundary. Startup and restoration still complete, and a prompt is
  accepted, while that fact stays unsupported: the harness has no stronger
  switch, and refusing the session would drop the binding the request
  constrains. Proof of the effective setting, including against that legacy
  managed layer, is what a supported report would require. It is not a startup
  gate. That contract is
  [ADR 0014](../../../docs/adr/todo/0014-nessa-owned-policy-hooks.md) and
  `codex_startup_and_restoration_continue_when_native_hook_suppression_is_unsupported`.

`AcpConfig.environment` carries `CODEX_HOME` as this agent's noncredential
context selector; `credential_environment` carries `CODEX_API_KEY` and
`OPENAI_API_KEY`. `NO_BROWSER=1` is set at launch: a gateway process has no
browser and nobody watching it, and signing in is the desktop's business, done
before an agent is offered at all.

Those two variables are passed through because they are the operator's to set,
not because they sign Codex in. The app-server the adapter runs builds its
authentication with the environment key switched off, so `codex login status`
answers "not logged in" on a machine where one of them is the only thing set,
and Nessa's readiness answers the same. The adapter will take a
`DEFAULT_AUTH_REQUEST` and sign itself in from that key at startup, which is why
this binding does not set one: that login writes the key, in plaintext, into the
user's own `auth.json` under `CODEX_HOME`, where it outlives the variable and is
then preferred to it. Setting `cli_auth_credentials_store` to `ephemeral` does
not avoid the write — checked against the pinned adapter rather than assumed. A
gateway starting an agent must not move an operator's credential onto the user's
disk, so signing Codex in remains `codex login`.

## Deleting a session

`CodexAcpProvider` implements `ProviderSessionDeleter`. The adapter advertises
`sessionCapabilities.delete` but answers `session/delete` by archiving the thread
(`threadArchive`): the transcript stays in Codex's store. A successful answer is
therefore reported as `ProviderSessionDeletion::Archived`, never as erased.

The adapter also advertises `sessionCapabilities.list`, answered from Codex's
`thread/list` without asking for archived threads, so an archived thread is not
listed. A deletion interrupted after Codex archived the thread, and before that
was written down, sends the delete again on its next try: if Codex archives an
already-archived thread without complaint the answer is `Archived` again, and if
it refuses, a refusal of a thread it does not list settles as
`ProviderSessionDeletion::NotListed`. Either way the deletion finishes. The
listing is also limited to Codex's current model provider and its usual thread
sources, so a thread Codex keeps under another provider is not listed either,
and is asked to be deleted the same way.

## Web search observation ordering

The pinned 1.12.0 harness sends `webSearch` identity at announcement and fills
`query`/`action` in the completed `tool_call_update`. These two fields describe
the completed search observation; they do not replace the original input retained
for permission review. The adapter owns this normalization in
`infrastructure/codex_acp/tools/wire.rs`. Tests below live in
`tests/infrastructure/codex_acp/tools/wire.rs`.

| Prior observation and arriving frame | Result | Regression |
| --- | --- | --- |
| No observation → search announcement | Retain original input and charge encoded bytes | `web_search_completion_can_supply_search_details_without_changing_review_input` |
| Unnamed, non-MCP search → completed update with same `webSearch` type, call ID, and stable fields, with query/action filled | Accept observation; original review input stays unchanged | `web_search_completion_can_supply_search_details_without_changing_review_input` |
| Completed search → duplicate matching completion | Accept without another input charge | `web_search_completion_can_supply_search_details_without_changing_review_input` |
| Search → changed input on an in-progress update, or changed identity on completion | Reject | `web_search_completion_exception_keeps_identity_and_command_inputs_strict` |
| Command, named tool, or MCP observation → changed input on completion | Reject | `web_search_completion_exception_keeps_identity_and_command_inputs_strict`; `web_search_completion_does_not_exempt_named_mcp_or_unknown_input_fields` |
| Search → changed type or additional stable field on completion | Reject | `web_search_completion_does_not_exempt_named_mcp_or_unknown_input_fields` |
| Search → permission request with changed input | Reject through the original exact-input comparison | `web_search_completion_can_supply_search_details_without_changing_review_input` |

This exception does not authorize a tool, expand its permissions, or change
retention accounting. Completion normalization and permission review have
separate owners: an observed completed search can gain metadata while an approval
must still describe the original request.

## Install and run

The local harness manifest and lockfile pin `@agentclientprotocol/codex-acp`
**1.12.0**, which bundles `@openai/codex`. Install from the repository root:

```sh
npm ci --prefix crates/nessa-sdk/harnesses/codex-acp --ignore-scripts --no-audit --no-fund
```

The desktop runtime ships one harness per agent, so the shared
`scripts/desktop/prepare-runtime.mjs` assembler installs this one beside the
Claude harness and records both versions in the manifest selected by the macOS
and Linux composers. Assembly refuses to publish a manifest when either harness
is incomplete. Linux service registration and release packaging remain disabled.

## Verification

On **October 11, 2026**, a source-built macOS gateway at upstream `fde8f6c`
plus the Web search fix in [#774](https://github.com/nessalabs/nessa-agent/pull/774)
ran the pinned 1.12.0 harness with an existing OpenAI sign-in and catalog model
`gpt-5.6-terra`. A native Swift product client used a non-admin panel credential
against an isolated loopback namespace. No credentials are part of this evidence.

| Live check | Observed result |
| --- | --- |
| Product authentication, conversation list, and agent discovery | Passed |
| Create, send, and read a text conversation | Completed turn and agent reply |
| Web search before this fix | `Protocol("tool input changed")` after the Running observation |
| Web search after this fix | Running and Completed observations for the same tool ID, then a completed turn and reply |

The checks inspect the gateway's durable provider observations as well as the
client's completed turn. They do not establish live permission approve/deny,
uploads, restoration, or every tool kind; those remain deterministic coverage
unless separately exercised. A machine without provider sign-in still stops at
the authentication boundary.

Deterministic coverage also includes: unit tests over the configuration and wire
mapping, and contract tests that drive a real subprocess speaking ACP —
`tests/infrastructure/acp/contracts/fixtures/codex_acp_test_handler.py` —
through the same shared runtime the Claude binding uses. Those cover the ordered
configuration steps, a harness or version that is not the pinned one, a model the
provider does not offer, a refused model or mode, terminal content arriving where
this client advertises none, a configuration notification arriving while the
session is still being configured, a permission request carrying its facts in
`_meta.codex` and both answers to it, an approval no audit sink could record, a
resumed session being configured again before it is used, the steering extension
being declined although the adapter offers it, the instructions reaching the
provider through its own configuration rather than the session request, and the
hook request (`features.hooks` false, `notify` empty) being present in
`CODEX_CONFIG` while the negotiated session reports native hook suppression as
unsupported, on the first open and again after restoration, with a prompt
accepted both times.
Resource-link content is covered by the wire-mapping unit tests rather than
through a subprocess.

Not covered under this profile, and covered under the Claude profile only: a
permission cancelled by a close or withdrawn by the provider, a caller lost
after its answer was admitted, and the queue-ordering paths. The transitions
belong to the shared worker and its own suites are thorough, but the standard
asks for shared guarantees to be run across the delivery modes that use them
rather than inferred from one profile, so these are named here rather than left
to be assumed. Running them across a profile and fixture pair, instead of
writing Codex copies, is what closes the gap.

Queue ordering is the narrowest of the three, and worth saying exactly. The
server routes a steer on the capability rather than on the agent — it queues
whenever `native_steering` is false — and the backend its own suites run
against leaves that at the conservative default, so the branch Codex takes is
already the branch those suites exercise. What is missing there is the name on
the fixture, not the behaviour. The gap that remains is in this crate's
suites, where the ordering is driven through a profile's own binding.

They are adapter and process tests. They establish that this binding speaks the
protocol it claims to and fails closed where it says it does; they establish
nothing about the real provider's behaviour.
