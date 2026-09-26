# Codebase structure

The repository rules live in [AGENTS.md](../AGENTS.md) and the
[coding standards](../CODING_STANDARDS.md). This guide maps those dependency and
ownership rules to Nessa's current modules and changes as the codebase grows.

## Organization applies to every change

The [organization standards](../CODING_STANDARDS.md#organization-across-the-repository)
are required for all languages and layers. Source files, tests, scripts,
configuration, and documentation belong with the feature or boundary they serve.
Use shared locations only for responsibilities that are actually shared.

Keep the feature vocabulary consistent across those locations. Preserve existing
consistency boundaries, give each module a clear map, and update navigation and
callers when moving files. Review the resulting tree and verify affected links
and checks before considering the change complete. A new folder alone does not
establish an architectural boundary.

## Today

Nessa has a Rust desktop host (window, tray, settings, OS integration), a React
shell (chat surface, composer, avatar), and a background gateway process.
The conversation vertical has earned the split: the panel is a projection, and
the gateway owns commands through the SDK Agent. New contexts establish the
feature-first, role-below layout immediately, including their module map and test
locations. Existing SDK layout changes are tracked in [the SDK organization task](todo/sdk-context-first-organization.md).

## Target shape

```
src/                      composition root (`main.tsx`, `store.ts`)
  conversation/           product vertical (model / use cases / gateway / UI)
  session/                wire session to nessa-server (@nessa/client)
  panel/                  floating-window chrome (model / application / adapters / UI)
  host/                   injected OS features + the window seam
  diagnostics/            development page-console forwarding
src-tauri/src/
  diagnostics.rs          debug-only page-to-terminal diagnostic bridge
  <context>/
    domain/               rules, entities, value objects, events
    application/          use cases + ports (traits) the use case needs
    adapters/             storage, OS, IPC command handlers, clock
    contracts/            what other contexts and the frontend may see
```

The Tauri command layer is an **adapter**, not a home for logic. A
`#[tauri::command]` function should read like: deserialise, call one use case,
serialise. If it contains a decision, that decision belongs in a use case or a
domain object.

The frontend is likewise an adapter. React state models *what is on screen*; it
does not own product rules. When the shell starts encoding a rule ("a turn can
only be cancelled while streaming"), that rule has a home on the Rust side and
the shell reads the result.

Types carry meaning here as much as anywhere: a `SessionId`, not a `String`; a
`PanelWidth`, not an `f64`. The window geometry code in particular is a place
where a wrong number in the right slot compiles happily.

## The absences

The [coding standards](../CODING_STANDARDS.md) define the repository-wide gates.
These additional Nessa-specific invariants must stay true:

1. `domain/` contains no `tauri`, `serde`-transport, async-runtime, or
   filesystem import.
2. `main.rs` is the app composition root, and `composition.rs` is what it
   assembles: one `HostDependencies` holding the settings file, the shortcut
   cache, the surface credential, the registered gateway, and the release
   source. Resolution happens at entry points — `setup`, a command's
   `State<HostDependencies>`, a handler that captured the bundle or resolves it
   once at the top — and the logic below them takes explicit parameters. OS-specific
   hosts are injected by `platform::current()`; shared modules never construct a
   macOS or Linux host. Live objects (menu items, the summon registration slot,
   the pending update, the announced release, the startup settings snapshot)
   stay managed state: they
   are not read from outside the process and have nothing to substitute.
3. The panel frame is reapplied on every show. Nothing caches a frame across
   shows — that is the bug the design exists to prevent.
4. Every host call goes through `src/host/window.ts` and no-ops outside Tauri.
5. A settings file missing keys still launches; every new key has a default.
6. Edge failures — blur, sizing, tray, viewport — are reported and survivable,
   never fatal. The panel opening unblurred, or without a tray, beats the panel
   not opening.
7. The page's viewport does not move during a resize, on macOS or on Linux.
8. Linux is reachable without a menu bar: skip-taskbar is off, and the panel
   opens on launch.
9. On Linux the webview's grandparent is the GtkWindow. Nesting the pin
   widget inside tao's vbox panics on click (Tauri unwraps that grandparent
   as a Window) and GTK cannot unwind, so the process aborts.

## The core and what sits on it

Nessa will grow a core — the agent runtime, the turn lifecycle, the transport to
whatever produces replies — and a set of product surfaces on top of it: the
panel, the composer, the tray, whatever comes after.

- The runtime core has no knowledge that a menu bar panel exists. No `if panel`,
  no field only the tray sets, no enum variant named after a surface.
- A surface composes core pieces and adds its own rules. It depends on the core;
  the core never depends on it.
- When a surface needs something the core cannot express, the core gains a
  *general* capability — a port, an event, a parameter naming a concept the core
  already has — and the surface supplies the specific part.
- The test: could this core piece serve a Nessa with no menu bar at all — a CLI,
  a second window, a background run? If not, it has been contaminated.

The same applies inside the host today. `panel.rs` owns the frame; `tray.rs`
owns the menu. The tray does not own conversation state, and it does not decide
preferences — it requests and reflects. Keep new code pointing the same way.

On Linux the tray may not exist. That is an edge failure, not a different
architecture: the panel is still the product, the taskbar is how you reach it,
and `main.rs` is still the only place that decides to show the window because
the tray is missing.

## Rules for the host/shell seam

The one boundary that already exists, and the one most likely to rot silently.

- **One definition of every payload shape, imported by both sides.** Never
  redeclare the shape of a command argument or event on the receiving side. Two
  declarations of the same contract drift with no error anywhere — the compiler
  is happy on both sides right up until runtime. The names and `PanelSize` live
  in `host.rs` and `src/host/window.ts`; a test in `host.rs` fails if a name on
  the host is missing from the shell. Generating one side from the other is the
  next step if this list grows.
- **Every new host call goes through the existing seam and no-ops outside the
  desktop host**, or `pnpm dev` breaks quietly for whoever does design work next.
- **Pass the first payload in, do not make the shell ask for it.** State the
  shell needs to paint its first frame should arrive with the shell, not as a
  round trip after mount. Defer anything heavy that is not needed for that frame.
- **Anything per-frame is scaled by elapsed time.** Panel animation, coasting,
  easing, decay — a flat per-tick multiplier runs at double speed on a 120 Hz
  display and produces bug reports that read "feels wrong on my machine". Write
  decay as a power of elapsed time, and velocity as pixels per second.
- **Fake the clock in tests.** Anything timed gets its clock injected, including
  animation and anything that measures itself.
- **Linux is not a later port.** A change that only works behind a macOS
  `cfg` — a tray that is fatal, a size command that errors, a frost that never
  paints, a resize that jitters — is a defect, not a platform gap.

## Failure-first checklist for host code

The general questions are in the skill. What they mean here, before any new code
touching the filesystem, the OS, or another process:

- What if it runs twice — two shows, a double shortcut press, a restart
  mid-write?
- Is this failure fatal or survivable? Match the existing policy: edge failures
  degrade the surface and are logged with the `[nessa]` prefix; they do not stop
  the launch.

Settings writes are the current instance of most of this: a partial write must
not produce a file that fails to load, which is what `serde(default)` plus
writing the full defaults on first launch is buying.

## Frontend specifics

- A component either renders or coordinates, never both. Coordination lives in a
  hook; rendering takes props and has no idea where they came from.
  `src/panel/ui/app.tsx` is the chrome. Conversation UI lives in
  `src/conversation/ui/`. Host subscriptions live in `src/panel/adapters/`.
- The Messages tab is panel chrome over a conversation list, like the update
  tab: `src/panel/application/messages-tab.ts` owns its place in the strip and
  its transitions, and `src/panel/ui/messages-tab.tsx` puts
  `src/conversation/ui/conversation-list.tsx` in it. The list is the gateway's
  (`conversation.list`), joined to the open tabs by
  `src/conversation/application/queries/roster.ts`; choosing a row shows that
  conversation's own tab or reopens it as one, so a thread is drawn in one place.
- The composer's vertical budget belongs to the composer, not to any notice.
  Five producers say things above the pill — a link that went nowhere, an
  update, the draft's files, the session, the conversation — and none excludes
  the others, so the column used to push the pill out of a short window.
  `src/panel/ui/composer-notices.tsx` owns both halves of that: the room they
  may have, which is a third of the panel and scrolls (`.nessa-composer-notices`
  in `src/styles.css`), and the order they are said in. Nothing is dropped or
  collapsed. The queue badge and the delivery row stay outside the box, pinned
  above the pill, because they are controls rather than statements.
  Two checks hold it up, and they are apart because of where each can run. The
  JSX half is the `nessa/composer-notices` lint rule
  (`scripts/eslint/composer-notices.mjs`, on for the chrome alone, tested by
  `pnpm lint:rules`): it refuses a notice added as a new direct sibling of the
  box or handed to another child that renders it, and a notice inside another
  module is that module's business. The stylesheet half is
  `scripts/architecture/composer-budget.mjs`, which reads every rule that names
  the box and refuses a ceiling deleted, overridden by a heavier selector or a
  media query, beaten by a `min-height`, or left without a scrollbar; it reads
  selectors, not the cascade. It stays pure text because
  `check-architecture.mjs` runs on the Rust jobs with bare Node and no
  `node_modules`, where nothing may import a parser.
- Product commands live in `src/conversation/application/usecases/`. The store
  is a projection: thunks call injected effects and reducers apply returned views.
  The shared tabs are `conversations` + `activeId`; beside them the
  `conversationHistory` slice holds the gateway's list of conversations, which
  is what the gateway holds rather than what this window has open. A tab's name
  is the gateway's (its view carries it) unless somebody renamed the tab. Local
  id counters live on
  `LocalTabs`, not on the model the future server will share. Other modules
  import `src/conversation` (the barrel), not files under it — `store.ts` is
  the exception, so tests do not pull the design system. Host subscriptions
  stay in adapters. See
  [adr/0002-conversation-vertical-and-gateway.md](adr/done/0002-conversation-vertical-and-gateway.md).
- Design-system components are consumed, not wrapped "just in case". A wrapper
  with no behaviour is a layer that only forwards.
- Decisions live in `model/` and `application/` and are tested as plain
  functions; rendering is tested as markup through `react-dom/server`. An effect
  that only React can run — one host call per mount, focus placement, cleanup
  on unmount — is tested in a file that opts into the DOM with
  `// @vitest-environment jsdom`, mounts with `react-dom/client` and React's own
  `act`, and wraps in `StrictMode` because `main.tsx` does. The global test
  environment stays `node`: the CI gateway harness copies `vitest.config.ts`
  with a fixed dependency list, and every other test needs no DOM.
  `src/onboarding/ui/use-setup-handoff.test.ts` is the example.
- Host-window interaction goes through one seam (as it already does), so the UI
  runs in a plain browser with the seam no-oping. Keep that property: it is what
  makes design work fast, and it is a real architectural boundary, not a
  convenience.
- Persisted UI preference is state with an owner. The frontend owning the
  surface choice and the tray reflecting it — rather than the tray owning it —
  is the right direction; keep new preferences pointing the same way.
- Frost is a host concern. macOS uses a native effect view; Linux and the
  browser use CSS. The shell picks via `data-host`, it does not reach for
  `backdrop-filter` on the macOS Tauri window.

## Dependency composition

Use the [typed DI foundation](design/dependency-injection.md). TypeScript constructs
one dependency scope in `main.tsx`, injects effects into Redux thunks, and shares
its session handle with the lifecycle. Rust composes `RuntimeDependencies` into
`AppState`; application-owned traits define replaceable effects. Extend these
patterns for actual backend integrations without adding a service locator.

## Agent SDK foundation

`crates/nessa-sdk` groups source and tests by layer, then feature. The
[crate module map](../crates/nessa-sdk/README.md#ddd-layers) is the detailed source
index; the [execution guides](../crates/nessa-sdk/docs/agent_execution/README.md)
own current lifecycle and API contracts.

| Location within the SDK | Responsibility |
| --- | --- |
| `domain/common/value_objects/` | Shared validated dates, URLs, and token limits. |
| `domain/model_metadata/`, `domain/effective_capabilities/` | Model catalog invariants and immutable admission capabilities. |
| `domain/agent_execution/` | Sessions, execution ordering, tools, permissions, and prompts; DDD roles beneath each feature. |
| `application/agent_execution/agents/` | Public Agent, scheduling, submission retry recovery, and one lifecycle owner for work generations, active work, and shutdown. |
| `application/agent_execution/providers/`, `hooks/` | Injected execution ports, operation capabilities, and typed invocation callbacks. |
| `application/agent_execution/sessions/` | Local session identity, exclusive storage lease, retained attachment resources, and snapshot evidence mapped through domain history rules. |
| `application/agent_execution/executions/`, `permissions/`, `tools/` | Domain coordination, attributed decisions, and observation/review projections. |
| `infrastructure/acp/`, `claude_acp/`, `codex_acp/`, `opencode_acp/` | Shared transport lifecycle, and one module per provider for its own configuration and tool translation. Verification shared by more than one provider moves up into `acp/`, as ordered session configuration did once Codex and Opencode both needed it. |
| `infrastructure/session_storage/` | Memory snapshots, incremental JSONL file persistence, and explicit evidence serialization. |
| `infrastructure/json_rpc/`, `process.rs`, `model_metadata_json.rs` | Framing, process supervision, and model catalog parsing. |
| `infrastructure/clock.rs` | The clock every ACP protocol deadline is measured on: `RuntimeClock` from composition, and `tests/infrastructure/manual_clock.rs` in tests, which moves only when the test moves it. |
| `tests/{domain,application,infrastructure}/` | Matching invariant, public orchestration, and storage boundaries. ACP tests live in `tests/infrastructure/acp/` and are included by the library through a test-only path declaration to exercise crate-private controls; Python handlers stay beside those contracts under `fixtures/`. |

Composition chooses models, provider configuration, storage, and the required
permission audit sink. Agent owns admitted work; UI adapters and gateway code call
its application contracts. Keep provider JSON, clock reads, filesystem access,
and processes out of the domain. Do not create empty counterpart modules or split
a live session's tool/permission consistency boundary into independent aggregates.

## Identity and access library

`crates/nessa-auth` is a workspace library with pure domain models and
application-owned DTOs/ports. See its [module and collaboration guide](../crates/nessa-auth/README.md).
The local backend, embedded Cedar, and `/session` gateway are implemented.
`nessa-server/src/product` owns the guarded wire profile; composition injects its
providers. See [local authentication](adr/done/0010-local-authentication.md). Hosted
identity providers remain future adapters.

`application/credential_registry.rs` owns the secret-free refusal facts and
audit port for a registry authority file that cannot be trusted. The local
registry adapter keeps one content-validation path and translates
private-storage refusals at the same open boundary: it reports the exact path,
the registry-or-lock role, and a bounded syntax, schema, invariant, size, or
storage fault and never rewrites the rejected file. The application derives the
role-specific preserved transition, so a lock refusal does not claim registry
state was read. `adapters/local/registry_refusal_audit.rs` records the target,
before/after meaning, cause, and known initiator outside the untrusted registry.
It creates and publishes only beneath the verified auth root, syncs each parent
after creating its child on Unix, applies the Windows guarantees described
below, and reports a sink failure beside the primary fault.
Server composition supplies whether the open came from gateway startup,
automatic provisioning, or an explicit local command.

`crates/nessa-agent-credentials` is the smaller pure domain consumed by the
gateway credential-source adapter and desktop credential writer. It owns
the immutable validated credential text, API-key/OAuth meaning, the explicit
Claude/OpenCode credential identity, and the durable stage/instance namespace.
It owns no keychain, environment, provider, serialization, or filesystem code;
the gateway keeps those effects behind its own application port. See the
[crate map](../crates/nessa-agent-credentials/README.md).

`crates/nessa-local-storage` owns native OS private-file mechanics shared by the
local auth, SDK session storage, and desktop credential adapters. It has no auth/domain policy
or Tauri dependency; callers inject the resulting adapters through composition.
Each primitive comes in two forms: a path-based one for a directory whose whole
path the caller trusts, and a `_beneath` one that walks a relative path down
from an already-verified root, refusing anything that is not a private
directory of this user's and never following a symbolic link. A caller whose
tree can be written to by anything else uses the second.
`create_private_directory_tree_beneath` establishes a nested private tree one
name at a time. Unix syncs each parent before descent and the final leaf before
success. Windows revalidates the anchored tree because it has no directory-fsync
equivalent; sinks flush record files and use write-through moves separately.
`PrivateDirectory` is the narrower retained-authority API for a store that scans,
locks, opens, and publishes repeatedly in one directory. It preserves the original
root, directory chain, and native identities; each enumeration owns an independent
non-atomic cursor, and every file operation takes one native component. Publication
never replaces a name and returns the still-open destination handle. A failure after
rename retains that published fact separately from its failed acknowledgement, while
a pre-rename cleanup failure stays separate from the primary failure. On Unix the
caller must hold its stable lock across each residual leaf check/effect interval; on
Windows non-delete-sharing handles pin the directory chain. Binding checks are
acknowledgement checkpoints rather than continuous attachment. Windows `sync` only
revalidates binding and does not claim directory durability or arbitrary power-loss
survival. See the [local-storage module map](../crates/nessa-local-storage/README.md#module-map).

On Unix, `create_private_directory_path` and `PrivateDirectory::open_path` retain an
absolute private store through ordinary locator ancestry. System and account
ancestors may use normal read and execute modes when they are owned by root or the
current user and are not group/other writable; the private leaf remains owned by
the current user at mode `0700`. Creation, parent synchronization, traversal, and
later binding checks all use directory handles without following links.

`crates/nessa-local-database` opens one private SQLite file for one context and
nothing else: a private directory and file through `nessa-local-storage`,
foreign keys, a rollback journal synced in full, `secure_delete`, and the
context's schema at the one version its definition states, refusing any other.
It has no tables of its own and no migrations; each context owns its schema, and
a schema change ships its own move. See the
[local-database crate](../crates/nessa-local-database/README.md) and
[ADR 196](adr/todo/196-conversation-metadata-database.md).

`crates/nessa-gateway-endpoint` owns the bound local endpoint, per-process
identity, application publication/discovery ports, and private-file adapters.
Its immutable domain values live under `domain/value_objects/`: `endpoint.rs`
validates the listener and process identity, while `advertisement.rs` enforces
agreement between that endpoint and optional desktop-managed identity. Tests
mirror those responsibilities under `tests/domain/value_objects/`.
File adapters require an already-created, current-user-private data root and
resolve the stage and instance namespace beneath that root without following
links. Server credential provisioning establishes that root on a new install;
an existing permissive or redirected root is refused.
Server composition publishes the actual listener address atomically beside
`gateway.log`; local Rust clients accept it only when every identity field agrees
with a bounded unauthenticated `/health` response. This correlation rejects stale
and mismatched listeners but is not authentication. `nessa-server` and the desktop
host compose the shared ports without depending on one another, and the Node
client is held to the same canonical record by the cross-runtime fixture in
`protocol/fixtures/gateway-endpoint.json`.

`crates/nessa-images` fits one image to a consumer's limits: it reads the
encoding from the bytes, turns the image upright, scales it down, and converts or
compresses it to PNG or JPEG, or says by type why it could not. It knows nothing
about agents, models, or conversations, does no I/O, and keeps no state. It reads
the common encodings with its own decoders and hands what only an operating
system reads well (HEIC, AVIF, JPEG XL, PSD, camera RAW) to a `PlatformDecoder`:
ImageIO on macOS under `src/platform/`, none yet elsewhere, where those are
refused by type. Two gates stand in front of a system decoder: `src/sniff.rs`
puts to it only bytes that begin like one of those encodings, on every system
and in front of a test's substitute, and the macOS adapter decodes only what
ImageIO itself names as one of them, so a PDF is refused rather than rendered.
`src/budget.rs` holds the one pixel and memory budget every decoder is held to
before a pixel is read, and an image that would pass through unchanged is
decoded whole first (`src/jpeg.rs` reads a JPEG strictly). Tests substitute the
decoder; `tests/memory.rs` covers the budget from headers alone and
`tests/macos.rs` runs the real decoder. The
numbers are the caller's: the gateway takes them from the selected model's
`imageInput` entry in the SDK catalog, the one place image limits are recorded.

## Gateway conversation ownership

`crates/nessa-server/src/conversation/` groups durable conversation identity/access
(domain), shared Agent orchestration and bounded views (application), and private
metadata/audit adapters (infrastructure). A conversation records the agent it was
created on and is reopened on that same agent for the rest of its life, so
`composition/agent.rs` builds fixed providers for configured bundled agents.
`composition/opencode_profile.rs` owns one static OpenCode decision shared by
configured/default selection, image limits, readiness, and provider creation.
Packaged composition either selects the exact current pin with a stage-scoped
API key or refuses the policy; standalone composition preserves its explicit
command, arguments, model, tools and budgets and captures `OPENCODE_API_KEY`
once when composition starts. `composition/current_agent.rs` joins that static
decision to a fresh launch and credential observation for each readiness call
and cold slot. Managed observations resolve the exact pinned launch and current
stage-scoped API key; standalone observations retain the captured environment
credential. An existing live slot keeps the generation and executable-use
authority it already owns. `ConversationAgentSource` is the application port
between those decisions and the service. Yesterday's
conversations still reopen on their recorded agent even when it is not today's
default. `product/conversation.rs` maps the canonical product wire contract;
composition supplies the agents, storage and audit.
The live slot owns a prepared SDK `Agent` before provider attachment. It captures
caller-attributed attachment authority, returns create/read/queue commands without
waiting for runtime readiness, and retains one bounded task that joins readiness
before starting the SDK-owned attachment. The SDK scheduler remains the only queue
on both sides of attachment. Replacement views project its `Waiting` and `Starting`
phases as `starting`, and retain typed, bounded late attachment failures.
Tests follow those responsibilities under `crates/nessa-server/tests/conversation/`.
A message refers to images by digest, never by bytes: the service asks its
`ConversationAttachments` port whether this conversation holds each one before it
accepts the message, and `close` releases what the conversation holds in the
closer's name, reporting a failed release without hiding a failed agent close.
The [attachments context](#attachments) implements that port.
A message may also point at files on this machine by path, which is how anything
that is not an image travels: nothing is uploaded, so there is no hold to check
and no ownership to verify, and the whole of what the gateway checks is that the
path means the file that was chosen, through the SDK's `LinkedFile` — absolute,
no control character, no component that a URI would drop or resolve away. It
checks nothing about the markdown link the agent is handed the path inside;
`prompt_content.rs` owns that, by encoding the two strings it interpolates
rather than by naming characters a path may not hold. What it records instead is who
pointed the agent there — `ConversationFileLinkAudit` and its
`DurableConversationFileLinkAudit` adapter, one record per conversation and
submission, committed before the message is admitted and written only for a
submission the agent has not already seen — a repeat is the SDK's to settle, and
recording one would let a conflicting retry write evidence naming paths it never
delivered. The record is evidence of the naming, which is intent: not a read,
not a delivery, not even an admission. A sink that cannot take it refuses the
send, and a sink already holding different paths for that submission refuses it
too. See [ADR 0013](adr/done/0013-files-by-path-not-by-payload.md).
A list of conversations is read without opening one, and without reading anybody
else's: `conversation.list` asks `ConversationListing` for the caller's own
conversations, newest first and one past the bound. `LocalConversationStore`
(`infrastructure/store.rs`) answers it, and is the repository and the
`ConversationSummaries` store too: ownership records, tombstones and summaries are
three tables of one private SQLite file, `conversations/metadata.sqlite3`, defined
once in `infrastructure/schema.sql` and opened by `crates/nessa-local-database`
([ADR 196](adr/todo/196-conversation-metadata-database.md)). A summary holds each
conversation's title, last line said and time, derived by `domain/value_objects/conversation_summary.rs` — the one
owner of those rules — when a message is accepted and when a reply completes. A
summary is a projection, so a failed write is logged and the command stands; the
`archived` flag it also carries is a person's decision, so `archive`/`unarchive`
fail visibly instead. `delete` is a consequential transition: a tombstone the
repository owns (a `deletions` row) fences the identity before the agent
is stopped, a `ConversationDeletionAudit` record (`infrastructure/deletion_audit.rs`,
`audit/deletion/`) is committed before history is erased, and the SDK journal is
erased under a lease the delete holds. Before that, the agent is asked to delete
its own session: `ProviderSessionErasers` (`application/provider_sessions.rs`) is
the one authority, a registry keyed by agent and filled at one site in
`composition/agent.rs`, and each agent's handler lives in its own SDK module
(`claude_acp`, `codex_acp`, `opencode_acp`) over the shared ACP exchange in
`acp/sessions/deletion.rs`. The ownership record and every audit store
are kept; see [ADR 182](adr/todo/182-conversation-deletion.md).
The floating panel uses injected conversation effects and NessaClient; neither
owns SDK scheduling. See [gateway chat](guides/gateway-chat.md).

Image input follows the same split. `@nessa/client` owns the wire: the
`AttachmentUploadTransport` port in `application/attachment-upload.ts`, its
`fetch` adapter in `transport/`, and `presentation/attachment-api.ts`. The
conversation vertical owns a draft file's upload state and the
`stageAttachment` effect, which answers with the reference the gateway stored;
the panel owns the original bytes and the order of one upload. Image conversion
and its limits belong to the gateway, so no TypeScript module scales, converts,
or compresses an image. A test outside a context imports that context's
`testing.ts` — the store's commands, the scenario substitute, typed errors, and
the pure parts of the barrel — rather than its internals, and mocks the barrel
with it when the barrel's components cannot be resolved.

## Agent runtime warm-up

`crates/nessa-server/src/agent_warm_up/` owns the one-time preparation of the
configured agent runtime: which runtime is being prepared (domain), running it
through one open and close and committing that evidence (application), and the
completion record and audit files in the data directory (infrastructure). It is
started by composition once the gateway is listening, so the operating system's
first-execution scan is paid in the background rather than inside a user's first
message. The application terminal keeps preparation, physical launch ownership,
audit delivery, and completion-record delivery as separate facts. The
conversation context joins settlement through its own `RuntimeReadiness` port
without blocking command responses, which `composition/warm_up.rs` connects;
the two contexts do not depend on each other. Composition coordinates one
automatic OpenCode run at a time: released failures may be retried, while
unconfirmed cleanup retains its owner and fences another automatic launch. The
conversation's freshly resolved provider always supplies the authoritative
request result. Tests live under `crates/nessa-server/tests/agent_warm_up/` and
the composition seam under `crates/nessa-server/tests/composition/`.

## MCP tools

`crates/nessa-mcp/src/mcp.rs` owns stdio transport and routing. Nessa-owned tools
live under their feature name with domain, application and infrastructure owners;
`shell/` contains command validation, runner/audit ports and the Shepherd adapter.
All additional Nessa tools use this MCP boundary. See the
[server guide](../crates/nessa-mcp/README.md).

`src/conversation/adapters/agent-stream/` maps replacement gateway projections to
Nessa UI AgentEvent/TranscriptBuilder. `ui/agent-transcript-view.ts` derives one
turn-level activity row from the shared Transcript; `ui/turn-activity.tsx` opens
its ordered thought and tool detail. Neither parses provider wire formats.
Runtime-owned declined-review notices use their own ordered `local_notice` part
and stable execution-scoped identity. The gateway upserts selection with its later
local write evidence before the agent-stream adapter renders it; it is not a
permission decision, provider message, tool failure, or execution status.
`ui/transcript.tsx` renders a turn's terminal status once at row level,
independently of whether that turn contains text.

### Packaged gateway lifecycle

- `scripts/desktop/native-window-smoke.mjs` owns the Linux WebKitGTK end-to-end
  boundary. Its adjacent `native-smoke-*` modules isolate executable discovery,
  process cleanup, WebDriver request lifecycles, failure evidence, and the
  deterministic ACP provider so each boundary can be tested without launching
  the native window. Those script tests run in bare Node with no installed
  packages. `src/host/load-fallback.test.ts` owns the embedded fallback's real
  DOM and computed-style clipping regression in the frontend jsdom gate.
- `scripts/desktop/stage.mjs` resolves one named stage for the Tauri command and its Vite child. Vite records the stage beside the assets it builds; `src-tauri/build.rs` resolves Tauri's effective base, platform, and `TAURI_CONFIG` layers and reads that record from the `build.frontendDist` Tauri will embed. It refuses a frontend whose stage differs from the host bundle stage, and the host accepts only an equal runtime `NESSA_STAGE` override.
- `scripts/desktop/prepare.mjs` selects runtime preparation on macOS and Linux;
  Windows leaves it disabled.
  `runtime-layout.mjs` owns the executable names used by assembly, signing, and
  bundle verification. `prepare-runtime.mjs` owns the shared native-target check,
  locked ACP harness installation, model data, and manifest publication after
  platform finalization and fingerprinting. `prepare-node.mjs` owns the pinned,
  hash-verified Node archive and safe extraction of Node plus its license for both
  platforms. Its cache objects are named by the pinned digest and published once
  from a verified private stage. Existing invalid, oversized, or nonregular
  objects are preserved and refused with their exact path; preparation never
  repairs or removes them. Exclusive publication coordinates cooperating build
  processes, while a same-user process with cache write access can still replace
  an object after one invocation has returned. `prepare-macos.mjs` supplies Apple signing; `prepare-linux.mjs`
  requires native x86_64 GNU/Linux and probes each executable without starting a
  service. Linux local builds bundle that prepared resource, but the Linux
  `GatewayHost` and release targets remain disabled. Windows has no preparation.
- `scripts/desktop/runtime-fingerprint.mjs` identifies that complete prepared tree, including model data and installed ACP dependencies; its adjacent tests cover content, layout, and relocation.
- `src-tauri/src/gateway/application/` is the single owner of retryable startup, its revisioned projection, retained cleanup candidate, and automatic-quit dispatch authority. `gateway/domain/value_objects/lifecycle_journal.rs` validates the immutable `Intent`, `JoinedRequest`, `EffectPlan`, `EffectCompletion`, `Observation`, and `Outcome` chain with contiguous sequence, one namespace and target, operation-specific agreement between old identity, desired target, label and cleanup scope, increasing observation versions, and a distinct physical result. No outcome can settle a plan whose primary completion or observation remains unresolved, and terminal cleanup must agree with the physical result. `gateway/infrastructure/reconciliation_audit.rs` acquires the private config root, journal child, and retained directory as one transaction, retaining exact rollback authority for each newly created component until the retained binding succeeds. It then holds one stage-wide lock while it validates every final record, writes deterministic immutable names, and derives a delivery receipt only after retained-directory sync, binding and exact file read-back. Before a current attempt opens, the host settles the sole restored intent or missing observation/outcome; each native adapter decides which fresh state it closes on (the domain requires only an observation for an intent with no plan), and recovery retains the exact pending step and completion. The journal lists the steps an interrupted attempt must settle; macOS recovery settles them without running any command, adopts only an exact planned target, and otherwise closes failed, reading launchd and health through the injected `Launchctl`. Every launchd read and command in the macOS adapter goes through that one seam — status, health, bootstrap, bootout, and the agent-stop and retirement signals — with `NativeLaunchctl` as the production adapter; the stale, legacy and unavailable registrations leave through one journaled `unload_service` step. Runtime and data directories are created only inside the acknowledged staging plan; fresh staging predeclares exact fingerprint cleanup. Newly created private ancestry is rolled back deepest first only while each retained name still has its original identity and remains empty. Bootstrap likewise predeclares an exact bootout contingency. Each contingency is owed only when the journal holds its step's result and observation and that result is not success; the live path runs it only then, after fresh proof for a bootout that the intended target still owns the label, and runs nothing after a failed delivery. The next successful registration prunes what an interrupted attempt left, and a matching staged runtime is reused under `reuse-staged-runtime`. LaunchAgent definitions are converted through pipes, written to exclusively reserved private files, and published with anchored replacement before the directory durability fact is recorded. Quit records `DesktopQuitPolicy`/`DesktopHost`, carries one monotonic deadline through lock acquisition, dispatch, and each terminal-outcome delivery attempt; the outcome owner retains a returned error or panic, any retry denial or second failure, and the settled physical result together. The session atomically consumes a matching identity/version proof immediately before `launchctl`; manager acceptance and the fresh post-command observation remain separate. Concurrent startup callers still share one receipt owner, while physical reconciliation and audit delivery remain independent facts. `gateway/infrastructure/commands.rs` exposes startup state only to bundled windows. The macOS adapter distinguishes managed, legacy, and foreign processes and requires the expected health fingerprint, generation, runtime-instance UUID, and launchd PID before readiness. `macos/generation.rs` allocates service generations; unfinished bootstrap authority remains in the same stage lifecycle journal.
- `src-tauri/src/gateway/infrastructure/linux/` owns the packaged Linux systemd user service. It refuses hosts without the account user manager, logind linger, the owned `UnitPath`, the root-owned non-symlink `/usr/bin/env` launcher, or pidfd support before admitting an intent, and it repeats the UID, desktop-injected XDG config/data/state roots, linger/pidfd prerequisites, manager owner, and path authority checks before native effects. Every refusal before intent acknowledgement states that Nessa changed neither the service nor linger. Linux lifecycle journals live under `XDG_STATE_HOME` (or `~/.local/state`), while definitions and immutable runtimes remain under their XDG config and data roots. It publishes a private immutable runtime and an acknowledged rendered-definition digest, plans data-directory creation, then publishes an identity-checked `default.target.wants` link and confirms enablement through `GetUnitFileState`. The unit passes the configured data root as `NESSA_DATA_DIR`; the server adds the stage and instance, so both sides name the same data directory. An installed exact owned unit that is inactive/dead with no process and no endpoint advertisement — a server that recorded a startup failure it will not retry and exited zero — is admitted like a fresh install with its installed bytes as the prior definition; on that path and a fresh install, each definition, reload, and start effect first confirms the unit is absent or has no process; every other installed unit without a corroborated endpoint is preserved. Definition and link publication predeclare transaction cleanup, run it even when completion delivery fails, retain cleanup failure beside the audit failure, and remove only the exact owned unpublished temporary. Start and stop use non-replayable `fail` jobs: the returned manager owner, unit, operation, mode, object path, and numeric job ID must agree with the plan and matching `JobRemoved` signal. Audit delivery failure after enqueue cannot cancel terminal waiting or fresh observation. The command result and a fresh typed physical state are separate facts: start succeeds only for the exact active target with native manager, invocation, PID, definition, endpoint, and link evidence, judged once the forked server advertises, the unit leaves `activating`/`running`, or the ready deadline passes, while stop succeeds only for absent or inactive/dead state. Replacement keeps the old link and runtime, journals the retirement request identity, opens a process handle through the injected pidfd seam immediately after snapshot A, requires the gateway's durable request/result acknowledgement tuple, then checks snapshot B and liveness immediately before signaling that held process. Quit uses the same seam and carries the handle through corroboration to the consume-once proof and signal. Recovery uses the admitted definition digest rather than current candidate inputs, records closed systemd states, closes an intent with no plan on whatever it freshly observes because no plan authorized no effect, closes settled plans on their last durable observation, treats an exact owned render of another target as a definition not yet replaced, never replays an unacknowledged D-Bus enqueue, and preserves ambiguous names and identities for diagnosis.
- The agent's `PATH` is decided, not inherited. `src-tauri/src/gateway/domain/value_objects/search_path.rs` is the value: absolute entries only, bounded, no control characters, and no staged runtime directory, so Nessa's bundled `node` never shadows the one a project pinned. `gateway/infrastructure/login_shell.rs` reads it from the account's own login shell — the shell named in the account record rather than inherited `SHELL` — behind the `LoginShellPath` port, which tests substitute. How a shell is asked depends on how much one invocation of it can read, measured against real shells. zsh is asked `-i -l -c` once, because for zsh that is a superset: it reads `.zshenv`, `.zprofile`, `.zlogin` and `.zshrc`, and `.zshrc` is where pnpm's installer and the standard nvm setup write. bash has no such invocation — `-i -l -c` reads `.bash_profile` and never `.bashrc`, `-i -c` reads `.bashrc` and none of the login files — so bash is asked both ways and the answers are combined. macOS puts the login answer first to match Terminal; Linux puts the interactive non-login answer first to match its desktop terminal policy. Asking only one of them would succeed with a `PATH` missing whichever half the user's tools are in, and a success is what no fallback can catch. Any other shell gets `-l -c`, which is also where the login files come from when nothing else has brought them: a `.bash_profile` that hangs while interactive leaves the `-i -c` answer alone, and that answer has read neither `/etc/profile` nor `.bash_profile` — no `path_helper`, so no Homebrew. Returning it as a success would be the same failure in a narrower place, and would make the registered path depend on whether a profile happened to hang, which the plist equality check answers by retiring a healthy gateway. Every attempt shares one budget, so a shell asked more ways is not a shell the panel waits longer for; the cost is that an attempt after one that timed out gets what is left rather than a full deadline. The shell runs from the account's home, not from wherever the app was started, so a profile that decides the path from the working directory cannot make two launches register differently. The `PATH` is printed between markers made of `/dev/urandom` bytes and only what is between them is read, so a chatty or hostile profile can neither drown the answer nor forge one. The shell runs with a cleared environment, no stdin, a bounded read, and one deadline over both the output and the exit — output arriving is not the shell being finished with — after which its process group is killed and reaped. `Gateway` resolves it at most once per host process and caches the outcome, failure included: reconciliation runs on every webview load, and a profile edited mid-session would otherwise change the definition and retire a healthy gateway. A healthy registration for the same staged runtime keeps its registered path across app launches, so shell-profile or launch-context changes do not retire it. A newly staged runtime takes the current resolved path. The path is registered as `NESSA_AGENT_PATH` in the native service definition; a login shell that cannot be read and a shell that changed both leave a healthy same-runtime definition alone. The service's own `PATH` stays the system one. `crates/nessa-server/src/composition/agent.rs` gives `NESSA_AGENT_PATH` to the ACP child — and through it to Claude Code's Bash tool and the Nessa MCP shell — falling back to the process `PATH`, which is what the developer loop has.
- `protocol/defaults/gateway-exit-codes.json` is why the gateway process stopped, said across the process boundary. `crates/nessa-server/src/core/exit_code.rs` maps each `RunError` to a code from that table — the match is exhaustive, so a new fatal error does not compile until it has one — and `Termination::report` exits with it; `RunError::Registry` keeps the credential-registry failure typed rather than flattened into a string so it can choose its own, and `RunError::Dataset` does the same for a gateway-scope store holding what this build cannot read — conversation metadata at another version or not a database, a browser-session journal replay refuses (`datasetRefused`, never retried; [ADR 202](adr/todo/202-versioned-local-datasets.md)). `src-tauri/src/gateway/infrastructure/macos/startup.rs` includes the same bytes and reads the code back from what `launchctl print` reports as the service's last exit. It never parses the log: a message can be reworded, a line can belong to an earlier run in the same append-only file, and a healthy launch mentions the same subsystems a failing one does. The log tail goes to the app's log as evidence and decides nothing.
- Not every failure is worth restarting for, and launchd cannot be told which. Its only exit condition is `KeepAlive: { SuccessfulExit: false }` — restart unless the process exited with status zero — with no condition on *which* non-zero code, verified against launchd on macOS 26 rather than read off the man page alone. So `crates/nessa-server/src/core/restart.rs` decides, on the typed fatal error, whether starting again could end differently: a configuration this build cannot parse, a credential registry it cannot read and a prepared runtime that is missing or not the one the registration was fingerprinted against cannot, while a held registry lock, a taken port, I/O and anything whose only evidence is prose can.
- Exiting zero to stop those restarts is not a diagnostic decision, because nothing will start the service again afterwards except the desktop host's next reconciliation, and the only thing that authorizes *that* is `logs/gateway-startup-failure.json`. So `core/error.rs` publishes the record — durably, under its own name, directory synced — and only then chooses the ending: published means exit zero, and a publication that failed keeps the non-zero code from the shared table, because launchd going on retrying is the old loop and survivable while a silent exit zero is a service nobody can start again. A credential-registry refusal audit is separate evidence: its success never authorizes zero, and its failure stays beside the original registry fault while the managed recovery record independently decides the ending. `core/launch.rs` decides who may do any of this: `Launch::Managed` is resolved once at the process edge from the command launchd is configured to run plus the service generation only the host's plist sets, and only it bounds the log, publishes a record, or forgets one — and only one its own generation wrote. A `nessa server` someone runs in the same data directory while diagnosing exactly this problem is `Standalone`: it keeps the table's non-zero exit codes, so scripts and other supervisors still read a failure as one, and it leaves the registration's recovery evidence untouched. `macos/startup.rs` corroborates the record against the registration being reconciled before it decides anything: it ends the readiness wait for a process launchd is not going to replace, and it is what lets the host boot out and retry a service that gave up, which is otherwise loaded and dead forever even after its cause is repaired. The reason and the exit code it carries must agree with the shared table, or it describes no run the server could have had. A record shown in a sentence is corroborated too, but decides nothing.
- Zero is also what a process that served and was asked to stop would ordinarily exit with, and under this `KeepAlive` that would permanently disable the service: a gateway killed from Activity Monitor or with `kill` would never come back, and only a hand-run `launchctl bootout` could make it startable again. So `core/ending.rs` gives a managed launch's clean stop the table's `stoppedOnRequest` code instead, and launchd brings it back a throttle interval later — which is what it did before the exit status meant anything. `launchctl bootout`, which is how that service is actually stopped, unloads the job before the status is read, so the service that is meant to stop still stops; both were verified against launchd on macOS 26. A standalone server still exits zero when someone stops it.
- `crates/nessa-server/src/core/log_file.rs` keeps `gateway.log` within a size bound at every start: over the limit, the contents go to `gateway.log.1` and the log is emptied in place. launchd opens that file itself and hands the process the descriptor, so renaming it would strand both descriptors on the renamed inode and leave `gateway.log` missing until the next spawn; emptying the same inode keeps them. launchd opens `StandardOutPath`/`StandardErrorPath` with `O_APPEND` (verified: `F_GETFL` reports `0xa` on fds 1 and 2 of a launched job), so the next write lands at the new end rather than back at the old offset. Only the process whose own stderr is that file rotates it, matched by device and inode. A single long-lived run can still outgrow the limit before its next start; there is no rotation thread. The subscriber asks for colour only when stderr is a terminal, so a log launchd wrote carries no escape sequences.
- Every advisory file lock is released by unlocking, never by closing alone. An `flock` belongs to the open file description, so a subprocess forked while one is held inherits a descriptor onto that same description and keeps the lock until it execs — `O_CLOEXEC` closes the descriptor there, not at the fork. Closing ours would leave the file locked by a `uuidgen` or a `launchctl` that has no interest in it. `NamespaceLock` (gateway reconciliation), `Journal` (browser sessions), `RegistryLock` (credentials) and the SDK's session `Lease` each unlock on drop for that reason, and `control.rs` has a test that reproduces the shape with `dup` rather than a fork to race against.
- One gateway per stage and instance is enforced three times over, and each layer catches what the one above it cannot. launchd allows one loaded service per label, and the label carries the stage and instance. The per-label reconciliation flock in `Library/Application Support/Nessa/gateway-locks/` serializes two hosts reconciling the same label. The credential registry's own lock — taken with `try_lock_exclusive` and held for the life of the store, over a registry that is itself per stage and instance — refuses any second gateway process, including a hand-started `nessa server` that launchd knows nothing about. That refusal has its own exit code (`alreadyRunning`), because reported as a registry fault it reads as corruption when it is the exclusion working.
- An update never runs two gateways at once. A stale managed service is asked to retire over `SIGUSR2`, must durably acknowledge with matching identity before anything else happens, and is only then booted out; the replacement is bootstrapped after that. A retirement that is not acknowledged returns an error with the old service still running, rather than booting out on a hope.
- `startup.rs` reads `launchctl print`'s exit fields in the shapes launchd actually prints, checked against it rather than assumed: `last exit code = 0`, `(never exited)`, a bare number, a sysexits number annotated as `78: EX_CONFIG`, `last terminating signal = Segmentation fault: 11` (which replaces the exit code line rather than joining it), and `last exit reason = JETSAM_…`. A missing program is launchd's own `78: EX_CONFIG`, not the 126 or 127 a shell would report, so that is what names an unlaunchable runtime. A signal outranks an exit code, and a signal line that cannot be read resolves to unknown rather than falling back to a code it contradicts.
- Readiness stops waiting once three consecutive half-second checks agree the process exited and was not replaced, well inside launchd's five-second restart throttle, so a crash loop reports in about a second and a half instead of at the seventy-five-second deadline. A live process may take sixty seconds to answer and still succeeds. The port, launchd and the clock reach it through `ServiceWatch`, so the timing is tested rather than asserted: a fake advances a virtual clock when the loop sleeps and records the instant of every `launchctl print`. That covers a slow-but-healthy start keeping its whole deadline, a crash loop giving up inside two seconds, a process returning between deaths restarting the count, and the subprocess staying spaced at the liveness interval rather than running every poll. Changing any of the three constants fails those tests. The panel gets one sentence, naming the cause when the server named one, and the exit status, log tail and readiness message go to the app's log. An exit line the host cannot read resolves to unknown, which never shortens the wait; the deadline is unchanged for a process that is alive.
- `src-tauri/src/gateway/infrastructure/macos/staging.rs` copies the bundled runtime to private immutable per-label/fingerprint directories, removes removable bundle-supplied extended attributes, syncs both cloned and byte-copied files, verifies full-tree parity with the packaging digest, and publishes atomically before service mutation. The injected launchd adapter remembers a successful validation only for the exact path, fingerprint and private root directory filesystem identity, so later reconciliation does not hash the full tree again; a different or replaced generation is validated normally. Existing versions are retained; launchd arguments name the staged directory and no search path derives from it — the gateway addresses `nessa`, `node`, the ACP entry and `nessa-mcp` absolutely, so the staged directory is on neither the service's `PATH` nor the agent's. Tests cover cache identity, cross-language Unicode/framing parity, private permissions, symlinks, rejected special entries, normalized file metadata, corrupt/existing versions and interrupted attempts.
- `src-tauri/src/gateway/infrastructure/macos/pruning.rs` collects the staged versions nothing can be running, under the same per-label lock, once the service has advertised its identity. `removable` is the whole rule and is pure: a published fingerprint directory goes only when it is neither the registered nor the running version and neither an unanswered retirement request nor an unacknowledged fence names it. `RetirementEvidence` carries `retired` for exactly that distinction: admission fencing needs only a recorded cause, while collection needs to know whether the old gateway finished and was booted out. An interrupted `.staging-` attempt goes because holding the lock means nobody is staging; every other name is left alone. `RuntimeVersions` is the directory seam, so the rule is tested without a filesystem and the real `LabelDirectory` revalidates each entry as a directory this user owns before removing it. No single entry can stop the pass: a refused removal, an entry the directory will not yield, and an entry with no valid text name are each reported and stepped over while the recognised versions beside them are still collected. Only a directory that cannot be listed at all ends the pass. What is reported is typed rather than a message string, so each line says what actually happened: a path appears only for an entry a removal was really attempted on, and a lossy rendering of an unusable name is never presented as somewhere to look. Failures are reported with their path and never reach registration's result. Reconciliations that return an error collect nothing, because the version they were replacing may still be running.
- `crates/nessa-server/src/desktop_runtime/` owns validated upgrade correlation, the admission-and-cleanup retirement use case, and private request/result/audit files. A managed old gateway stays alive until it has durably acknowledged retirement; launchd performs replacement only after that acknowledgement.
- `crates/nessa-server/src/composition/desktop.rs` bootstraps private local access and injects bundled provider paths. Managed OpenCode launch resolution remains live in `composition/current_agent.rs`.
- `settings.stopAgentsOnQuit` controls agent cleanup on desktop exit; launchd owns gateway lifetime independently.
- `settings.onboarding.completed` records that first-run setup finished. `src-tauri/src/main.rs` opens the setup window only when it is false. `panel::finish_setup` owns the whole handoff and its order — show the panel, record completion, close the setup window — in the process that outlives that window; a panel that will not show abandons the handoff and writes nothing, while a refused write is logged and the close still happens. `src/host/window.ts`'s `finishSetupWindow` is a single invoke of it, carrying only whether setup was finished or left (`isOnboardingCompleted`), and maps the reported steps onto the `SetupHandoff` outcomes. `src/onboarding/application/setup-recovery.ts` decides what the setup window shows when it is still there afterwards: a panel that never came up offers the handoff again, a panel that came up over a window that would not close offers only that window's close. The `Destroyed` handler in `main.rs` is a safety net for dismissal and crashes, not the handoff's cleanup path. The debug-only tray item clears the flag through `panel::restart_onboarding`.

### Updating the app itself

- `src-tauri/src/updater.rs` asks the release endpoint in `tauri.conf.json`
  (`plugins.updater`) once per launch, spawned from the end of `main.rs`'s `setup`
  so it cannot delay the panel or first-run setup. Its `offer` is the whole rule
  and is pure: only a newer published version produces anything, while being
  current and a check that did not complete both produce nothing at all. A failed
  check is expected (an offline machine) and goes to stderr, never to the screen.
- The panel is the only surface, and the tray has no update item at all. A found
  release is kept in `updater::Announced` and sent to the panel window alone
  (`host::UPDATE_AVAILABLE`); the panel also asks for it on mount through
  `updater::available_update`, because the check can finish before that page has
  listeners — or while the panel is closed, in which case the update simply waits
  there. Both read the one value the host holds. No dialog, prompt, or window is
  involved, and setup is a different window, which is what keeps the check safe
  to run while setup owns the screen.
- `src/panel/application/update-surface.ts` owns everything the panel decides:
  the notice above the composer, the tab the install opens, the download's line,
  and the dismissal. It is pure and tested beside itself. Dismissal is scoped to
  a version and to a launch — it lives in the panel's own memory and is never
  written down, so a newer version notices again and the same one does not until
  the app restarts. `src/panel/adapters/use-update.ts` is the wiring: three host
  subscriptions, the question on mount, and the invoke that starts the download.
- The check, the download, and the install all run in the host process, so the
  webview neither calls the updater nor reaches the endpoint: `install_update`
  is an app command, so no capability grant and no `connect-src` entry exist for
  it. `src-tauri/capabilities/` stays as it was.
- Progress comes from the plugin's own chunk callback, throttled by
  `updater::worth_reporting` to one event per whole percent — or, when the server
  declares no length, one per 256 KiB — so a real artifact does not send
  thousands of IPC messages at a bar with a hundred positions. A download the
  server did not measure draws an unmeasured bar rather than a made-up
  percentage.
- A refused install is the one update failure that reaches the screen, because it
  is the one somebody asked for: `host::UPDATE_FAILED` turns the tab's line into
  a plain statement and a retry, and the update goes back in its slot so the
  retry has something to download. A refused *check* stays on stderr.

#### Watching the update flow locally

Two recipes, and they stop in different places. Neither publishes anything and
neither needs a release build.

- **`NESSA_FAKE_UPDATE=<version> pnpm app` — the decision, with no network.**
  A debug build reads the variable and answers the check from it instead of
  asking the endpoint, so the panel's notice offers 9.9.9 within a second of
  launch, with no server, no artifact, and no signing key. It is the inner loop
  for `updater::offer`, the notice, the tab, and the dismissal.
  `updater::simulated` is the whole rule: a `major.minor.patch` of digits is
  announced, an unset or empty value is an ordinary run, and anything else is
  reported on stderr and falls through to the real endpoint — the value becomes
  the notice's own text, and "Update available / banana" claims something no real
  check could have found.

  Nothing is faked beyond the answer. There is no release behind the offer, so
  taking the install downloads nothing, installs nothing, and restarts nothing;
  it travels the same `UPDATE_FAILED` path a refused download does, so the tab
  says so rather than sitting at nought per cent. Everything this path needs
  — `SIMULATED_UPDATE`, `simulated`, `SimulatedReleases`, `SimulatedUpdate`, and
  the two call sites — is `#[cfg(debug_assertions)]` and is not compiled into a
  release build at all, the same shape as `tray.rs`'s `SHOW_SETUP_ITEM`. A
  shipped app that can be told an update exists is one an attacker can tell that
  to. The tests are gated the same way; `cargo clippy -p nessa-app --all-targets
  --release -- -D warnings` is what proves the release arm still builds clean
  with none of it present.

  It does not touch the endpoint, the manifest, the download, the signature, or
  the install. It is the panel and the decision, nothing else — including the
  tab's refused state, which is the one part of a real download it can show.

- **`--check-only` plus a `--config` merge on `dev` — the real plugin, a local
  server.** This is the more valuable of the two, because the code doing the
  work is `tauri-plugin-updater` itself rather than a substitute:

      node scripts/desktop/updater-harness.mjs --check-only
      pnpm tauri dev --config '{"plugins":{"updater":{"endpoints":["http://127.0.0.1:7430/latest.json"]}}}'

  `--config` merges on `dev` exactly as it does on `build` (verified against the
  CLI in this tree, `@tauri-apps/cli` 2.x), so this needs no new runtime switch
  and adds nothing to the shipped config. Check-only mode serves a well-formed
  manifest announcing `99.0.0` — above whatever `tauri.conf.json` says, so it
  reads as newer without lowering the build — and no artifact at all, which is
  what lets it skip the slow release build the full harness requires.

  Real, through the plugin: the HTTP fetch, the manifest parse including its
  RFC 3339 `pub_date`, the `{os}-{arch}` lookup, and the version comparison.
  Not reached, on purpose: the announced URL 404s, so the update tab ends in its
  refused state and signature verification never happens — the manifest's
  `signature` field is a base64 sentence saying so. Install and restart are
  untested here. The banner and the 404 log line both say this; a clean run is
  not an end-to-end pass. For the download, the signature, the install, and the
  restart, build a real artifact and run the harness without `--check-only`.

#### Verifying the updater before a release exists

Two layers, because the updater has one failure that cannot be repaired after
shipping: if `plugins.updater.pubkey` is not the public half of the key releases
are signed with, every shipped build rejects every update forever and the only
remedy is a manual reinstall by every person who installed it. No later release
can fix it, because no later release can be installed.

- `src-tauri/tests/updater_key_pairing.rs` is the key-pair gate, and it is fast
  and automatic. It signs a fixture with the release private key through
  `pnpm tauri signer sign`, then verifies that signature against the pubkey read
  out of `tauri.conf.json` — read, not copied, so the gate cannot drift from what
  ships. It verifies through `minisign-verify`, a `[dev-dependencies]` entry and
  the same crate `tauri-plugin-updater` resolves to at runtime, decoding the
  base64 wrappers and allowing legacy signatures exactly as the plugin's
  `verify_signature` does; a pass is the real verifier saying yes. Nothing about
  it is compiled into the app. `scripts/desktop/config.test.mjs` still checks the
  key's *shape*, which a well-formed key from the wrong pair passes.

  A *mismatch* fails everywhere, unconditionally. The private key is absent from
  an ordinary `cargo test`, and that prints a loud multi-line skip naming what
  went unverified rather than a quiet pass. Wherever the key is present — above
  all the release workflow, which holds it as `TAURI_SIGNING_PRIVATE_KEY` — set
  `NESSA_REQUIRE_UPDATER_KEY_PAIRING=1` and the absence becomes a failure too, so
  a release cannot be built on a runner where the secret silently went missing.
  A release job must run this test with that variable set before it bundles.

- `scripts/desktop/updater-harness.mjs` is the end-to-end harness, run on demand.
  It takes an artifact `createUpdaterArtifacts` already produced (it does not
  build one — that is slow and needs the whole bundled runtime), signs it, copies
  it clear of the bundle directory, writes a `latest.json` in the plugin's own
  shape, serves both from `node:http` on localhost, and prints the `pnpm
  app:build --config` command that builds an older app pointed at it. Its header
  says what a successful run looks like from the tray and separates the three
  failures being tested: endpoint unreachable, manifest unreadable or
  inapplicable, and signature rejected. `scripts/desktop/updater-manifest.mjs`
  holds the parts worth testing without a key or a server — the `{os}-{arch}`
  target key the plugin looks up, the manifest fields, the check-only manifest,
  the artifact locations — and `updater-harness.test.mjs` covers them.

  Redirection is a build-time `--config` merge and nothing else. The shipped
  `tauri.conf.json` keeps the GitHub endpoint and gains no switch: a setting that
  redirects the updater is a setting an attacker can redirect it with.

  What neither layer covers: that the release workflow signs with the key the
  gate was run against (run the gate *in* that workflow), and GitHub's release
  hosting, redirects, and TLS, which only a published release exercises.

The first update from a gateway that predates retirement acknowledgement uses a
single explicit legacy bootout after its sole listening PID matches the exact
loaded launchd service PID. A listener
found only by port is never stopped. Later updates exchange correlated records in
`gateway-upgrade/` under the running gateway's namespace, fence new conversation
commands, join admitted commands, close owned agents, record the transition, and
then replace the service definition. Failed or contradictory acknowledgement
preserves the old process. An admitted retirement cause fences admission even when cleanup or audit fails;
that evidence forces a stale-service retry and, with one exception, never
authorizes bootout. A result that did not retire names why, as `refusal` from
`protocol/defaults/gateway-retirement-refusals.json`. The exception is
`data_missing`: nothing the gateway started still holds resources, by the SDK's
own cleanup fact, and the gateway's conversation data is gone. The host then stops the
old service itself: launchd unloads it under its own plan,
`unload-unretirable-service`, and systemd stops the unit with its ordinary stop
step (ADR 221). A pending
request for the live instance/generation also forces retry if result publication
failed; stale requests for other instances or generations do not. Successful
results retain the original validated lifecycle principal, cause, and correlation as durable retirement fences;
fresh requests never delete them, and the host syncs a correlated success before
bootout. Installation failure preserves the desired registration and any loaded
replacement for forward recovery, without restoring an old definition or stopping
an unretired new process. UI retry starts reconciliation again rather than
reusing an earlier startup error.

### Automated dependency guard

`pnpm architecture` checks frontend vertical imports and explicit Rust imports in
SDK, auth, server, MCP, and desktop gateway domain/application modules. Domain imports cannot reach
application DTOs, adapters, runtime libraries, or OS effects; application modules
cannot import concrete infrastructure/composition. The guard and its negative
fixtures run in CI. It is a source-level import check, not a Rust module resolver:
macros, fully qualified expressions, transitive re-exports, lifecycle ownership,
and semantic DTO relationships still require compilation and review.

`scripts/check-runtime-dependencies.mjs` separately follows Cargo's resolved
package IDs from the server, SDK, auth, local-storage, local-database, images, and MCP packages.
It rejects reachable Tauri desktop-framework packages under the default and
all-feature workspace configurations, including renamed and transitive edges,
while allowing the unrelated desktop application graph. Cargo metadata includes
dependencies for every target; features are conservatively unified within each
queried workspace configuration. The check and its Cargo fixture run once in the
platform-independent `gateway-contract` job with bare Node and Cargo.

### New-context layout from day one

Use `context/domain`, `context/application`, and `context/infrastructure` for new
Rust features. Application owns ports/DTOs and orchestration; infrastructure
translates external protocols and effects. Keep UI and transport entrypoints as
outer adapters. The existing TypeScript vertical uses `model`, `application`,
`adapters`, and `ui` for those responsibilities.

Create concrete owners and module maps even for a small first feature. Layout
is not a reason to invent a domain entity, generic runtime, or registry: a host
integration with no domain rules can begin with application and infrastructure.
Place each next real rule in its prescribed role rather than leaving everything
in one catch-all file. New MCP shell code is the small Rust example; the desktop
gateway context demonstrates an application port with native adapters. Older SDK
paths remain current until the coordinated TODO updates all consumers.

## Browser sessions

`crates/nessa-server/src/browser_session/` owns browser sign-in: `application/`
coordinates current-origin admission and its asynchronous storage port,
`domain/value_objects/` owns immutable authoritative session state, the exact
structurally validated HTTP(S) origin, and the rolling idle lifetime used to
validate every stored session, and `adapters/`
implements the bounded session journal with filesystem work on the blocking pool
(and memory test adapter), and `entrypoint/http.rs` translates cookies and requests.
Tests under `tests/browser_session/` cover lifetime, persistence, and HTTP boundaries. Its module map
exports handlers and origin checks. Composition enables plain browser HTTP only
for a dev/CI gateway bound to numeric loopback. Server
composition chooses storage. The product socket retains mandatory authentication
and per-operation authorization for both native and browser sessions. Browser
storage retains only the credential ID, bound origin, and idle-lifetime evidence;
the auth registry resolves current identity and access state on every admission.

## Agent readiness

`crates/nessa-server/src/agents/` answers which coding agents could actually
start here, before there is a session to authenticate with.
`domain/value_objects/` owns `AgentId` — which carries the one name an agent is
known by outside the server, because configuration, this route, the socket and
the conversation records on disk must all spell it the same way — the host's
three-way `HostAnswer`, and the `Readiness` rule that turns two answers into one
thing to tell the person;
`application/` owns the `AgentProbe` port, whose typed `ProbeFailure` keeps "not
signed in" apart from "could not tell", the `ReadAgentReadiness` use case that
only asks and maps, and `SharedAgentReadiness`, which bounds what one
unauthenticated request can cost: concurrent callers share a single in-flight
probe (never a cached answer) and stop waiting for it at a deadline;
`infrastructure/local.rs` asks this machine about every configured agent, owning
only the order sources are asked in and what an unanswered source means, with the
launch files and config directories resolved once in composition;
`infrastructure/claude.rs` and `infrastructure/codex.rs` each hold
what is true of that agent alone — where it keeps a credentials file, whether it
has a keychain item or answers for its own store by being run — and
`infrastructure/credentials.rs` holds what makes a file a sign-in, which is the
same for both, so a third agent
gets a third sibling rather than a branch inside either; `entrypoint/http.rs` owns the wire
vocabulary and the cross-origin rule for `GET /onboarding/agents`, and answers a
reading it could not obtain with 503 rather than an invented readiness.
Composition injects `LocalAgentProbe` through `ProductDependencies`, and the
handler receives the shared reader over it alone via `FromRef`. Tests under
`tests/agents/` split domain rules, application orchestration, the shared
reader's bounds, the HTTP boundary, the local probe's failure modes, what makes
a file a sign-in, and each agent's own conventions.

OpenCode readiness and cold opening instead share `composition/current_agent.rs`.
Each managed call reads one exact managed launch and one scoped credential
snapshot; a standalone call combines its explicit launch with the environment
credential captured at composition time. Readiness maps that evidence without
carrying the secret, and a cold slot builds an owned provider from a fresh
observation. The blocking effects have one bounded lane whose native task retains
its permit after caller timeout or cancellation. The actual executable-use
admission remains in the SDK immediately before process spawn.

The same context defines `AgentCredentialSource`; one local adapter reads
standalone Claude environment credentials before the Nessa login-keychain item,
while preserving API-key versus OAuth meaning. Desktop composition writes a
validated API key to that same durable stage/instance account through its own
application port and records secret-free intent and outcome evidence. Its values
and account namespace come from
`nessa-agent-credentials`; `protocol/defaults/agent-credentials.json` is the one
infrastructure mapping for the Security.framework service and the Claude and
OpenCode item names. `CredentialedClaudeProvider` is the provider adapter that
reads the same injected source as readiness on a blocking worker for each process
open. Packaged OpenCode reads the source once per observation and accepts only
an API key, which it supplies to that owned process generation. Standalone
OpenCode never reads that source; it uses only the environment value captured
by its static profile.
Neither adapter puts the source value in settings, plist, arguments, audit records,
or logs.

## Attachments

`crates/nessa-server/src/attachments/` owns the files a conversation uploads so
its messages can refer to them. `domain/value_objects/` describe a file
(`Attachment`: digest, `MediaType`, size, which must all agree), the verified
`Caller` behind an action, and one `UploadTicket` with its five-minute
`TicketLifetime`; `domain/entities/` owns the `Hold`, one conversation keeping
one stored file and remembering the file that was uploaded to produce it;
`domain/aggregates/` owns the `TicketBook`, where single use, expiry, replacement
by the same request made again, withdrawal, and the bounds on outstanding
tickets (in all, per organization, per conversation) are decided together. `application/` owns
`AttachmentService` and its ports: `AttachmentStore` and `StagedUpload` for
bytes, `AttachmentAudit` for evidence, `ImageNormalizer` for turning an uploaded
image into the one that is kept, `ConversationOwnership` for asking who owns a
conversation without opening an agent, `TicketSecrets` for randomness, and
`UploadBody` for a transfer however it arrives. `infrastructure/store.rs` keeps
bytes once per digest under `attachments/blobs/` and one record per hold under
`attachments/holds/<sha256(organization)>/<conversation>/`, private, with every
name derived rather than copied from input and one lock ordering every change.
Holds live until their conversation is closed or deleted, and a hold written
for a conversation deleted while its bytes were still arriving is taken back;
`audit.rs` commits one private record per transition; `conversation.rs` and
`images.rs` implement the conversation context's `ConversationAttachments` and
the SDK's `UserImageSource` on top of this context. `entrypoint/http.rs` is
`PUT /attachments` and its preflight: it authenticates nobody, acts only on a
ticket the authenticated socket issued, streams the body, and answers with the
stored reference. `product/attachment.rs` maps `attachment.begin`.
Composition builds the store once in `composition/attachments.rs` and hands out
the service (shared by the socket and the route through a narrow `FromRef`), the
conversation port, and the image source; it is composed only when an agent is.
The normalizer is an argument of that factory: `infrastructure/normalizer.rs`
fits every upload that says it is an image to the selected model's `imageInput`
limits from the SDK catalog, through `crates/nessa-images`, reading the encoding
from the bytes and never from the declared type. Composition also hands it the
running system's image decoder, which is the one thing here that reads outside
this process, so a test can put a decoder that refuses, answers nonsense, or
stops dead in its place. A model with no recorded limits has no image prepared
for it, and `ImageNormalizer::offers_images` says so before a ticket is issued:
an `image/*` `attachment.begin` on such a gateway is refused with
`image_input_unsupported` rather than answered with a ticket for bytes no
message could name.
`src-tauri/src/attachments/` is the desktop half: the file a person picks, as
a path rather than as bytes. `FilePicker` is the operating system's own dialog,
`ChosenFiles` is the filesystem — a chosen file's kind, length and bytes —
`ContentTypes` is the platform's own type database, `AttachmentTickets` is
the desk the one-shot tickets are minted at and spent, `DragBoard` is the drag
pasteboard, and `Readiness` is the chooser that decides who can fetch a file
that is not on this disk yet. Separate ports, because they are separate outside
things: a dialog needs a window server, a disk answers about a path, a type
database is Launch Services or shared-mime-info answering about a format, a
ticket needs the operating system's randomness to mint and its clock to expire,
a pasteboard belongs to a drag session and is gone when that session is, and a
cloud service answers about a placeholder on its own schedule. The length and the contents stay together because they are the
same disk answering at the same moment, and only one double can stage a file
that turns out longer than it claimed. The ticket desk is a port for the same
reason and one more: it is where the rule that a page cannot name a path lives,
so a substitute has to be able to stage the two refusals no real desk can be
made to produce on demand — a ticket presented twice, and one presented too
late. All of them are built in `composition.rs` and injected.

The content type is what keeps the product rule honest. A dropped file is typed
by the platform through the browser; a picked one had nothing to type it, so it
was classified from a 24-entry extension table and every format the platform
knew and the table lacked — `.ico`, `.jpe`, `.svgz`, `.jp2`, `.xbm`, `.tga`,
`.dib` — uploaded when dropped and travelled as a path when picked. Now both
routes ask the platform first and fall back to the table, which is one call with
one set of inputs. macOS is untested and Linux has never been run; that is
stated in the module header too.

Dropping is the host's too, for one reason: `dragDropEnabled: true` is the only
way a dropped file's path can be known, and turning it on costs the webview
*every* HTML5 drag event rather than only the ones carrying files — wry's
listener returns `true` and the drop never reaches the DOM. So the page receives
no drag or drop events at all in the app, and `attachments/dropping.rs` supplies
what it lost: dropped files go through the same `describe_each` a picker
selection goes through, dragged text and web-page images are read off the drag
pasteboard (`attachments/dragged.rs`, snapshotted on `Enter`, because the
session's payload is gone by the time a drop event reaches a handler), a dropped
folder is walked here under `MOST_FOLDER_FILES` and `MOST_FOLDER_ENTRIES`
instead of by the page, and a `dragging` event tells the panel when to draw the
drop target.

Both gestures name themselves, through `Announce::began` — a drop when it lands,
a `+` selection the moment the picker's answer comes back — and that name is a
`Batch`: a private-field newtype minted in `attachments/batch.rs` and nowhere
else, so no answer can be built without one. Dropping `Default` alone had been a
speed bump, since a struct literal with `String::new()` still compiled.

A **drop** gives one of three answers, and two of them land on a draft: the
files and the refusal. Both travel back on the `Dropped` with the name attached,
and the panel routes both by it. The third is dragged text, which goes to the
focused composer and names no draft — so a drag carrying no paths is never
announced, and the name it is given anyway is never looked up. That is
`attach_begun_by`, and it is a rule with a test rather than a condition inside a
function that needs a window.

A **`+`** differs in what needs the name, not in where the name comes from. The
page begins that gesture, so it captured the draft at the press and passes it
straight to `addChosenFiles`; the name is what the *tile* needs, because the
host draws that mid-call and has nothing else to go on. `began` says
`gesture: "picked"`, and the panel answers with the draft it is already holding
rather than reading the open tab a second time — one capture either way. Two
reads of one fact agreed only while nothing could change between them, and what
guaranteed that was the picker being modal: rfd's presentation choice, not a
promise. `readiness.rs` has the seam test that pins `"picked"`, because that
literal is the whole of what makes the distinction work and nothing else checked
it.

The name exists because an attach can be three quarters of a minute behind its
gesture, and by then the open tab is no evidence of anything. Only the drop
announced itself for a while, so a `+` selection of two placeholders put its
second tile on whichever tab was open when the second file finished; and the
refusal branch read the open tab even for a drop that had a name, so a folder
refused late took a tile off one draft and told a different one about a file it
had never seen. Neither guesses now. A drag of text is deliberately not named:
it pastes into the focused composer and never looks a name up, and naming it
spent one of the sixty-four the panel remembers.

`attachments/readiness.rs` is the seam for a file that is not readable yet. A
file iCloud is keeping answers a `stat` with a real name, type and length and
has nothing behind it, and a path handed over for one of those is a path the
agent fails to open some minutes later with nothing on screen to explain it.
Handlers claim a file by asking the operating system what it is keeping — the
dispatch is on the file's *state*, never on its type, since the type already
decides the route and must go on deciding only that — and each owes a bound, a
typed outcome, and no history: once the bytes are here it is an ordinary local
file. There is an iCloud handler and a refusal; other File Provider extensions
are dataless in the same way and are not handled, because whether they
materialise on read is theirs to decide and nothing here can find out. While a
file is being readied the panel shows a named tile that does not claim the file
is attached and holds the send, and it goes on any outcome.

Nothing that is not a regular file is opened, and both the look and the read
carry deadlines on plain threads rather than the blocking pool, so a FIFO or a
stalled mount costs one thread instead of wedging the panel for the session. A
read is authorised by a one-shot ticket the picker minted, not by a path the
page names.
`attachment` and `attachment_bytes` are the whole rule and are pure: an absolute
path with its final component and its length, or bytes within the bound, or a
typed `NotAttached` refusal. A path that is not valid UTF-8 is refused rather
than lossily renamed, a file past `LARGEST_ATTACHMENT_BYTES` is refused before
it is allocated, and one unusable file refuses the whole selection instead of
shortening it silently; cancelling is an empty answer, not a failure. The page
calls `choose_attachment_files` and, with the ticket it answered with,
`read_attachment_bytes` through `src/host/window.ts`, and the host calls the dialog plugin, so
`src-tauri/capabilities/` stays as it was — the same arrangement as
`install_update`.

The reading exists because the file's type decides its route and the gesture
never does. An image is uploaded and normalised however it was attached, so the
images among a picker's answers are read back before they are staged, while
everything else travels as the path alone. `src/conversation/model/attachments.ts`
owns that decision in `declaredMediaType`, which for a file nobody opened has
only the name to go on.

Holds live until their conversation closes; what expires on its own is an unused
ticket. Tests under `tests/attachments/` split domain rules, the service over
doubles, the real store on a real filesystem, audit records, the adapters, the
HTTP boundary, the wire mapping, the socket and router, and the facts this
context and the published protocol schema must agree on (`agreement.rs`).

## Installing an agent runtime

`crates/nessa-server/src/agent_install/` puts an agent's own runtime on the
machine at the version Nessa has tested. All three agents are pinned, and
`install-agent` fetches and verifies any of them; Opencode is the only one that
is also *launched* from what was fetched, because the desktop still resolves
Claude and Codex inside the bundle. Their pins cover macOS on Apple silicon and
no other platform, which is where the 467 MB was measured and the only archives
anybody has listed. The packaged OpenCode profile starts only with a saved API
key and selects the metered `opencode/minimax-m3` Zen model; Nessa does not infer
account validity or make a paid call during setup. The broader reason applies to
every agent equally: telling somebody to go and install something before they
can use Nessa is the thing this context exists to avoid. [ADR 173](adr/todo/173-fetch-agent-runtimes.md) is the decision to
fetch all three and ship none.

`domain/value_objects/` owns what is true before any file exists: `AgentName`,
which is the identity in this context and is constrained to what can also be a
directory name; `PinnedRelease` and the values it is made of — `ReleaseVersion`,
`ReleasePlatform`, `ArchiveUrl`, `ArchiveDigest` and `ArchivePath` — each of
which refuses its own malformed spellings at construction, so a bad pin is a
failing test rather than a surprising install. `PinnedRelease::accept` is the
one comparison between what was pinned and what arrived.

It also owns what a build needs of a machine and what a machine has, in
`host_platform.rs`: `Libc`, `ReleaseRequirements` and `HostPlatform`. These
exist because a platform does not identify a binary — one Opencode version
ships as nine archives, differing in C library and processor baseline as well
as in operating system and architecture, and a glibc build does not start on a
musl-only machine. `PinnedRelease::runs_on` is the comparison, and
`ReleaseRequirements::demand` ranks two builds a machine can both run, which is
what makes the choice between them independent of the order the pin file lists
them in. Six of those nine are pinned: the three `-baseline` packages hold the
same bytes as their siblings at 1.18.31, so what they need is unsettled and an
x86-64 machine without AVX2 is offered nothing rather than a build nobody has
run on one.

Install evidence is domain state too. `install_transition.rs` owns the validated
request identity, target artifact, and immutable transition facts; its private
representation prevents a rejected digest from equalling the target, a runtime
from replacing itself, or a rollback from claiming the failed target was
restored. Incomplete recovery retains the original publication failure, each
cleanup stage that failed, and either the remaining state the store confirmed
or an explicit unconfirmed state. `value_objects/publication_delivery.rs` owns
immutable preparation, outcome, and settlement values; its private outcome
representation requires every terminal to pass through the install-attempt
sequence validation. `entities/install_attempt.rs` is the sequence
owner: started may become verified or rejected, and only verified may become
installed, replaced, rolled back, or incomplete recovery. Application and
infrastructure cannot construct contradictory before/after evidence around that
owner. The request's account and invocation identity plus the domain-owned
started, verification-outcome, or completion-outcome slot identify one logical
event. The same owner admits live and restored facts, accepts an exact replay
even after a later event, and rejects different facts in the same slot. A
record's random UUID, sequence, and observation time describe its physical
publication and do not change that semantic identity.

`application/` owns the order and none of the effects: `InstallAgentRuntime`
first recovers an admitted publication for the account and then resumes its
durable superseded-runtime work before it does
installed-already, download, hash, accept, publish, and never unpacks
an archive that was not accepted. Its ports are `ArchiveSource` (the network),
`RuntimeStore` (this machine's disk), `InstallAudit` (durable transition
evidence), `InstallationDelivery` (publication admission and recovery), and
`ReclamationAudit` (immutable evidence for each bounded removal attempt), with
`ReclamationOperationIds` as the injected source of fresh removal identities. The
delivery session holds one lock from recovery through settlement. It durably
prepares after verification and before publication, retains the exact terminal
independently of audit acknowledgement, and settles only a matching outcome. A
prepared publication with neither a retained terminal nor an exact journal
terminal blocks new effects; runtime state never supplies the missing fact.
`StagedArchive` carries an open file rather than a path, so the bytes
that are measured are the bytes that are unpacked. Publication captures the
prior valid artifact while holding the store's per-agent lock and returns that
authority as a lease. The use case keeps the lease through both immediate durable delivery attempts
and drops it when `execute` returns. A later invocation must recover and
acknowledge the retained terminal before it admits another install effect.
An exact replacement retains a domain-owned cleanup obligation and an
independent receipt naming the exact delivery preparation on that same lease
before publication settlement. Cleanup completion does not erase that receipt:
after settlement the delivery owner returns the exact settled fact, and only
then may the aggregate acknowledge the receipt and admit a successor
replacement. Missing or contradictory settlement evidence blocks successor
effects. One invocation makes at most one removal
attempt per eligible obligation, retains the physical outcome before audit,
and reports removal, persistence, and audit failures as separate typed facts
without reversing the successful install. An interrupted admitted effect
recovers as observation-only work, so restart cannot repeat a deletion whose
result is unknown.
Journal sequence and observation time remain observation order; domain event
identity and before/after facts retain causal meaning.
Audit failure stays visible and carries typed state evidence: unchanged, the
target installed, the prior artifact restored, no runtime installed, or
unconfirmed. It therefore does not turn an uncertain cleanup into a claim that
nothing is installed. It also retains the exact transition whose
acknowledgement failed. The original upstream store diagnostics are moved once
into the returned failure and remain full-sized; only their audit projection is
bounded. This does not cap allocations already made by a provider or store.
`retry_audit` redelivers only that transition and never
repeats download or publication. A normal `execute` reports a typed reused
request when its initial started event was replayed, before any install effect;
a genuinely new invocation uses a new request identity.
If publication durability fails, the store removes only the target it computed
from the accepted pin, restores only a prior record whose validated artifact
still had every private nonempty file named by its `ReleaseContents`, and
re-reads installed state under the same lease. The original failure, every
cleanup failure, and the confirmed or explicitly unconfirmed remaining state
travel as separate typed facts through the audit attempt and back to the caller
when the sink also fails.

`infrastructure/` holds the four outside things: `pinned_releases.rs` reads
`data/agent-releases.json`, compiled in so the tested version cannot depend on
what is beside the binary, and is also the one boundary that reads *the
machine* — `host_platform()` builds a `HostPlatform` from the compiler's own
target, the C library this binary was linked against and what the processor
reports, so everything above it chooses by comparing two values rather than by
asking the operating system; `https_archives.rs` fetches over HTTPS only,
through a bounded redirect chain and a bounded body; `managed_runtimes.rs`
keeps installed runtimes under one private directory, one directory per
artifact rather than per version, serialising publication behind a per-agent
lock and writing its record durably once the executable is really there. Every
path inside it is relative to that root and is reached through the storage
crate's `*_beneath` primitives, which walk down one verified component at a
time; the root itself, which composition owns, is the single path resolved the
ordinary way. So no symbolic link between the root and a runtime can send a
read, a write or a removal outside the store, and the installed executable is
private to its owner like everything else there. Managed launch resolution
also returns an inseparable executable-use authority: each spawn durably admits
one generation while holding a shared artifact lock. A bounded per-artifact
inventory is initialized before publication becomes launchable; each generation
has separate immutable expected, admitted, and released records, so a missing
inventory, orphan phase, reused identity, or interrupted admission is
conservative after restart. Cleanup releases only that generation after it
confirms the process tree is gone or no spawn occurred. Dropped guards, launcher
death, task cancellation, and failed release acknowledgement leave durable
evidence; physical cleanup confirmation remains separate from release
acknowledgement and never regresses when the latter fails. Reclamation holds the publication lock,
rechecks the current artifact, takes the superseded artifact lock without
waiting, and refuses current, busy, marked, linked, or unowned targets. It
removes only the complete retained `ReleaseContents` through anchored storage
operations. `runtime_reclamation/` persists the aggregate and one immutable
audit record per operation; retained admissions, physical outcomes, and audit
acknowledgements remain distinct restart facts. `audit/journal.rs` retains one
opened directory authority and the original lock-file identity, then takes a
fresh handle to that same lock for each record so CLI processes serialize one
monotonically sequenced journal. `audit/record.rs` owns its private JSON mapping
and reconstructs semantic facts through domain constructors. Exact replay
reopens and re-syncs the original immutable record under the same authority;
conflicting facts are rejected without claiming the incoming event was
published. The adapter syncs each record and the journal directory before
acknowledging it, and refuses corrupt, non-regular, non-canonical, or
discontinuous entries rather than appending past them. Regular files with the
storage primitive's exact private-reservation syntax are preserved and ignored:
the syntax is not provenance, and the journal neither promotes nor deletes an
abandoned reservation, so those files can consume disk until separate cleanup
is designed. Sequence and observation time describe journal observation order,
not domain causality. An error drops the runtime publication lease after both immediate delivery
attempts. The account-scoped delivery lock makes a later invocation recover the
earlier terminal before it can admit another install. Directory sync is unavailable on Windows, so its
power-loss guarantee remains limited to the storage primitive's documented file
behavior there.
`delivery/journal.rs` separately retains immutable preparation, outcome and
settlement records beneath `installation-delivery/agent-install`. Its stable
lock spans recovery, admission, the runtime effect, both terminal delivery
attempts and settlement. `delivery/record.rs` maps private JSON through the same
domain constructors used by the live path. Records are bounded before
publication and while read; settlement without its exact predecessor and
multiple unresolved attempts are refused. Records remain until separate
delivery-journal maintenance is designed. A replacement cannot settle until
its exact installation reclamation obligation has been durably acknowledged;
settlement does not erase the reclamation aggregate or its immutable audit.
The stable lock excludes every cooperating writer. On Unix it does not protect
the check/effect interval inside the journal leaf from a malicious process
running as the same user and deliberately ignoring that advisory lock; detected
directory, lock, or record replacement is still refused.

`composition/install_command.rs` wires those for `nessa install-agent NAME`,
picks the build for this machine — the most demanding of the pinned releases
that run on it — supplies a fresh correlation identity and the effective local
account whose private data receives the runtime, and reports one line of JSON on
stdout. After pin and platform admission, composition creates or verifies the
selected private data namespace before it constructs the audit and publication
delivery journal beneath that root; an unsafe namespace stops the command
before download or publication.
`scripts/agents/pin-agents.mjs` regenerates the pin file by downloading
and hashing every platform's archives. Composition reads the store through
`composition/installed_launch.rs` for each managed readiness observation and
cold conversation open. It answers with an owned exact-pin launch or with
nothing, never with a free-text path written down earlier. Tests under
`tests/agent_install/` split the domain's rules, application ordering and
failure reporting, the three adapters, concurrent publication authority, and
the command's output.

## Command-line surface

Developer worktree lifecycle is owned by `scripts/worktree.sh`. Manual sibling
worktrees and Claude Code's nested worktrees use different naming namespaces,
but both keep Cargo output in a real `target/` directory inside the checkout.
`scripts/worktree-target.test.mjs` covers creation, legacy migration, cleanup,
removal, invalid links, hook reopen behavior, and A/B/A artifact provenance.
The optional `sccache` process cache is the cross-checkout reuse boundary; Cargo
target directories are not shared by the recipe. `scripts/cargo-target.mjs` is
the one resolver used by scripts that build and then execute an artifact, so an
explicit `CARGO_TARGET_DIR` selects the same output for both steps.
The worktree clean command separately resolves that effective value and permits
deletion only when it is the invoking checkout's ordinary local target.

The `nessa-server` crate builds the `nessa` executable. `cli/entrypoint/` parses
commands, `cli/application/` coordinates token requests through its gateway port,
and `cli/infrastructure/` implements the bounded local WebSocket adapter.
`composition/cli.rs` wires the adapter, clock, identities and stdout/stderr.
`install-agent` is parsed here too — its agent name is read into `AgentName` at
this edge, so nothing further in holds a name it still has to doubt — and
`composition/install_command.rs` runs it on a blocking thread, because the work
is a download, a hash and an unpack and the HTTP client it uses declines to run
inside the async runtime. See [installing an agent
runtime](#installing-an-agent-runtime).
Tests mirror those responsibilities under `tests/cli/`; `scripts/smoke-auth.mjs`
checks actual process output and authenticated server effects.
`scripts/smoke-conversation.mjs` drives the real gateway and `@nessa/client`
through authentication, attachments, retry/reconnect, controls, persistence and
cleanup; `scripts/conversation-smoke/` supplies its bounded deterministic Claude
ACP process and evidence helpers. Offline bootstrap
remains in `composition/auth_command.rs`; it requires explicit `--local` selection.
Cloud auth is reserved but not implemented. See [local auth](guides/local-auth.md).
