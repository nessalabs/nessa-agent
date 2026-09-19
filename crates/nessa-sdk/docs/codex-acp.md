# Local Codex ACP binding

This provider guide covers the local Codex harness. Start with the
[agent execution guide](agent_execution/README.md) for domain ownership, lifecycle,
permissions, prompts, and shared transport contracts, and read the
[Claude guide](claude-acp.md) alongside it: everything the two bindings share —
process supervision, restoration identity, queueing and steering, the hook
contract, the credential and context environment split — is stated there and is
not repeated here. What follows is what is Codex's own.

## Composition and application boundary

Identical to the Claude binding in shape. The host supplies a selected
`ModelMetadata`, explicit `TokenLimits`, an `infrastructure::acp::sessions::AcpConfig`,
and an `Arc<dyn ExecutionAudit>` to `CodexAcpProvider::new`. The factory is
immutable, and `Agent::new(provider, manager)` opens or resumes a context.

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

`AcpConfig.environment` carries `CODEX_HOME` as this agent's noncredential
context selector; `credential_environment` carries `CODEX_API_KEY` and
`OPENAI_API_KEY`. `NO_BROWSER=1` is set at launch: a gateway process has no
browser and nobody watching it, and signing in is the desktop's business, done
before an agent is offered at all.

## Install and run

The local harness manifest and lockfile pin `@agentclientprotocol/codex-acp`
**1.12.0**, which bundles `@openai/codex`. Install from the repository root:

```sh
npm ci --prefix crates/nessa-sdk/harnesses/codex-acp --ignore-scripts --no-audit --no-fund
```

The desktop bundle ships one harness per agent, so
`scripts/desktop/prepare-macos.mjs` installs this one beside the Claude harness
and records both versions in the runtime manifest. A bundle missing either is
rejected at startup rather than configured around.

## Verification

**No live Codex check has been run for this binding.** What exists is
deterministic: unit tests over the configuration and wire mapping, and contract
tests that drive a real subprocess speaking ACP —
`tests/infrastructure/acp/contracts/fixtures/codex_acp_test_handler.py` — through
the same shared runtime the Claude binding uses. Those cover the ordered
configuration steps, a harness or version that is not the pinned one, a model the
provider does not offer, a refused model or mode, terminal content arriving where
this client advertises none, a configuration notification arriving while the
session is still being configured, a permission request carrying its facts in
`_meta.codex`, and the instructions reaching the provider through its own
configuration rather than the session request. Resource-link content is covered
by the wire-mapping unit tests rather than through a subprocess.

They are adapter and process tests. They establish that this binding speaks the
protocol it claims to and fails closed where it says it does; they establish
nothing about the real provider's behaviour. The dated live table in the
[Claude guide](claude-acp.md#verification) has no counterpart here yet, and this
guide gains one when somebody runs it, not before.
