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
src-tauri/src/
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
- The composer's vertical budget belongs to the composer, not to any notice.
  Five producers say things above the pill — a link that went nowhere, an
  update, the draft's files, the session, the conversation — and none excludes
  the others, so the column used to push the pill out of a short window.
  `src/panel/ui/composer-notices.tsx` owns both halves of that: the room they
  may have, which is a third of the panel and scrolls (`.nessa-composer-notices`
  in `src/styles.css`), and the order they are said in. Nothing is dropped or
  collapsed. The queue badge and the delivery row stay outside the box, pinned
  above the pill, because they are controls rather than statements.
  `scripts/architecture/composer-notices.mjs` refuses a notice added beside the
  box and a stylesheet that has lost the ceiling.
- Product commands live in `src/conversation/application/usecases/`. The store
  is a projection: thunks call injected effects and reducers apply returned views.
  The shared tabs are `conversations` + `activeId`. Local id counters live on
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
| `infrastructure/acp/`, `claude_acp/` | Shared transport lifecycle and provider-specific configuration/tool translation. |
| `infrastructure/session_storage/` | Memory snapshots, incremental JSONL file persistence, and explicit evidence serialization. |
| `infrastructure/json_rpc/`, `process.rs`, `model_metadata_json.rs` | Framing, process supervision, and model catalog parsing. |
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

`crates/nessa-local-storage` owns native OS private-file mechanics shared by the
local auth, SDK session storage, and desktop credential adapters. It has no auth/domain policy
or Tauri dependency; callers inject the resulting adapters through composition.

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
metadata/audit adapters (infrastructure). `product/conversation.rs` maps the
canonical product wire contract; composition supplies provider, storage and audit.
Tests follow those responsibilities under `crates/nessa-server/tests/conversation/`.
A message refers to images by digest, never by bytes: the service asks its
`ConversationAttachments` port whether this conversation holds each one before it
accepts the message, and `close` releases what the conversation holds in the
closer's name, reporting a failed release without hiding a failed agent close.
The [attachments context](#attachments) implements that port.
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
message. The conversation context waits through its own `RuntimeReadiness` port,
which `composition/warm_up.rs` connects; the two contexts do not depend on each
other. Tests live under `crates/nessa-server/tests/agent_warm_up/`.

## MCP tools

`crates/nessa-mcp/src/mcp.rs` owns stdio transport and routing. Nessa-owned tools
live under their feature name with domain, application and infrastructure owners;
`shell/` contains command validation, runner/audit ports and the Shepherd adapter.
All additional Nessa tools use this MCP boundary. See the
[server guide](../crates/nessa-mcp/README.md).

`src/conversation/adapters/agent-stream/` maps replacement gateway projections to
Nessa UI AgentEvent/TranscriptBuilder. `ui/agent-transcript-view.ts` derives activity
rows from the shared Transcript; it does not parse provider wire formats.

### Packaged gateway lifecycle

- `scripts/desktop/prepare.mjs` builds the macOS runtime resource tree from locked dependencies.
- `scripts/desktop/runtime-fingerprint.mjs` identifies that complete prepared tree, including model data and installed ACP dependencies; its adjacent tests cover content, layout, and relocation.
- `src-tauri/src/gateway/application/` owns retryable reconciliation; `gateway/infrastructure/` serializes launchd transitions, distinguishes managed, legacy, and foreign processes, and requires the expected health fingerprint and service generation, runtime-instance UUID and matching launchd PID before readiness. Its `macos/generation.rs` reuses the published identity only for an equal unfenced definition and otherwise creates a fresh random generation; configuration reverts never deterministically recreate retired identities. `macos/install_attempt.rs` durably binds a pending bootstrap to the host's exact definition and generation so Retry can replace only that incomplete, unambiguously PID-less attempt; disk state alone never grants that authority.
- The agent's `PATH` is decided, not inherited. `src-tauri/src/gateway/domain/value_objects/search_path.rs` is the value: absolute entries only, bounded, no control characters, and no staged runtime directory, so Nessa's bundled `node` never shadows the one a project pinned. `gateway/infrastructure/login_shell.rs` reads it from the account's own login shell — the shell named in the account record rather than inherited `SHELL` — behind the `LoginShellPath` port, which tests substitute. How a shell is asked depends on how much one invocation of it can read, measured against real shells. zsh is asked `-i -l -c` once, because for zsh that is a superset: it reads `.zshenv`, `.zprofile`, `.zlogin` and `.zshrc`, and `.zshrc` is where pnpm's installer and the standard nvm setup write. bash has no such invocation — `-i -l -c` reads `.bash_profile` and never `.bashrc`, `-i -c` reads `.bashrc` and none of the login files — so bash is asked both ways and the answers are combined, the login shell's entries first, since that is the shell a terminal opens here. Asking only one of them would succeed with a `PATH` missing whichever half the user's tools are in, and a success is what no fallback can catch. Any other shell gets `-l -c`, which is also where the login files come from when nothing else has brought them: a `.bash_profile` that hangs while interactive leaves the `-i -c` answer alone, and that answer has read neither `/etc/profile` nor `.bash_profile` — no `path_helper`, so no Homebrew. Returning it as a success would be the same failure in a narrower place, and would make the registered path depend on whether a profile happened to hang, which the plist equality check answers by retiring a healthy gateway. Every attempt shares one budget, so a shell asked more ways is not a shell the panel waits longer for; the cost is that an attempt after one that timed out gets what is left rather than a full deadline. The shell runs from the account's home, not from wherever the app was started, so a profile that decides the path from the working directory cannot make two launches register differently. The `PATH` is printed between markers made of `/dev/urandom` bytes and only what is between them is read, so a chatty or hostile profile can neither drown the answer nor forge one. The shell runs with a cleared environment, no stdin, a bounded read, and one deadline over both the output and the exit — output arriving is not the shell being finished with — after which its process group is killed and reaped. `Gateway` resolves it at most once per host process and caches the outcome, failure included: reconciliation runs on every webview load, and a profile edited mid-session would otherwise change the definition and retire a healthy gateway. A changed profile takes effect at the next app launch. The resolved path is registered as `NESSA_AGENT_PATH` in the launchd definition, so it is part of the service's identity and changes only by re-registration; a login shell that cannot be read this launch keeps the path already registered rather than rewriting the definition and retiring a healthy gateway. The service's own `PATH` stays the system one. `crates/nessa-server/src/composition/agent.rs` gives `NESSA_AGENT_PATH` to the ACP child — and through it to Claude Code's Bash tool and the Nessa MCP shell — falling back to the process `PATH`, which is what the developer loop has.
- `protocol/defaults/gateway-exit-codes.json` is why the gateway process stopped, said across the process boundary. `crates/nessa-server/src/core/exit_code.rs` maps each `RunError` to a code from that table — the match is exhaustive, so a new fatal error does not compile until it has one — and `Termination::report` exits with it; `RunError::Registry` keeps the credential-registry failure typed rather than flattened into a string so it can choose its own. `src-tauri/src/gateway/infrastructure/macos/startup.rs` includes the same bytes and reads the code back from what `launchctl print` reports as the service's last exit. It never parses the log: a message can be reworded, a line can belong to an earlier run in the same append-only file, and a healthy launch mentions the same subsystems a failing one does. The log tail goes to the app's log as evidence and decides nothing.
- Not every failure is worth restarting for, and launchd cannot be told which. Its only exit condition is `KeepAlive: { SuccessfulExit: false }` — restart unless the process exited with status zero — with no condition on *which* non-zero code, verified against launchd on macOS 26 rather than read off the man page alone. So `crates/nessa-server/src/core/restart.rs` decides, on the typed fatal error, whether starting again could end differently: a configuration this build cannot parse, a credential registry it cannot read and a prepared runtime that is missing or not the one the registration was fingerprinted against cannot, while a held registry lock, a taken port, I/O and anything whose only evidence is prose can.
- Exiting zero to stop those restarts is not a diagnostic decision, because nothing will start the service again afterwards except the desktop host's next reconciliation, and the only thing that authorizes *that* is `logs/gateway-startup-failure.json`. So `core/error.rs` publishes the record — durably, under its own name, directory synced — and only then chooses the ending: published means exit zero, and a publication that failed keeps the non-zero code from the shared table, because launchd going on retrying is the old loop and survivable while a silent exit zero is a service nobody can start again. `core/launch.rs` decides who may do any of this: `Launch::Managed` is resolved once at the process edge from the command launchd is configured to run plus the service generation only the host's plist sets, and only it bounds the log, publishes a record, or forgets one — and only one its own generation wrote. A `nessa server` someone runs in the same data directory while diagnosing exactly this problem is `Standalone`: it keeps the table's non-zero exit codes, so scripts and other supervisors still read a failure as one, and it leaves the registration's recovery evidence untouched. `macos/startup.rs` corroborates the record against the registration being reconciled before it decides anything: it ends the readiness wait for a process launchd is not going to replace, and it is what lets the host boot out and retry a service that gave up, which is otherwise loaded and dead forever even after its cause is repaired. The reason and the exit code it carries must agree with the shared table, or it describes no run the server could have had. A record shown in a sentence is corroborated too, but decides nothing.
- Zero is also what a process that served and was asked to stop would ordinarily exit with, and under this `KeepAlive` that would permanently disable the service: a gateway killed from Activity Monitor or with `kill` would never come back, and only a hand-run `launchctl bootout` could make it startable again. So `core/ending.rs` gives a managed launch's clean stop the table's `stoppedOnRequest` code instead, and launchd brings it back a throttle interval later — which is what it did before the exit status meant anything. `launchctl bootout`, which is how that service is actually stopped, unloads the job before the status is read, so the service that is meant to stop still stops; both were verified against launchd on macOS 26. A standalone server still exits zero when someone stops it.
- `crates/nessa-server/src/core/log_file.rs` keeps `gateway.log` within a size bound at every start: over the limit, the contents go to `gateway.log.1` and the log is emptied in place. launchd opens that file itself and hands the process the descriptor, so renaming it would strand both descriptors on the renamed inode and leave `gateway.log` missing until the next spawn; emptying the same inode keeps them. launchd opens `StandardOutPath`/`StandardErrorPath` with `O_APPEND` (verified: `F_GETFL` reports `0xa` on fds 1 and 2 of a launched job), so the next write lands at the new end rather than back at the old offset. Only the process whose own stderr is that file rotates it, matched by device and inode. A single long-lived run can still outgrow the limit before its next start; there is no rotation thread. The subscriber asks for colour only when stderr is a terminal, so a log launchd wrote carries no escape sequences.
- Every advisory file lock is released by unlocking, never by closing alone. An `flock` belongs to the open file description, so a subprocess forked while one is held inherits a descriptor onto that same description and keeps the lock until it execs — `O_CLOEXEC` closes the descriptor there, not at the fork. Closing ours would leave the file locked by a `uuidgen` or a `launchctl` that has no interest in it. `NamespaceLock` (gateway reconciliation), `Journal` (browser sessions), `RegistryLock` (credentials) and the SDK's session `Lease` each unlock on drop for that reason, and `control.rs` has a test that reproduces the shape with `dup` rather than a fork to race against.
- One gateway per stage and instance is enforced three times over, and each layer catches what the one above it cannot. launchd allows one loaded service per label, and the label carries the stage and instance. The per-label reconciliation flock in `Library/Application Support/Nessa/gateway-locks/` serializes two hosts reconciling the same label. The credential registry's own lock — taken with `try_lock_exclusive` and held for the life of the store, over a registry that is itself per stage and instance — refuses any second gateway process, including a hand-started `nessa server` that launchd knows nothing about. That refusal has its own exit code (`alreadyRunning`), because reported as a registry fault it reads as corruption when it is the exclusion working.
- An update never runs two gateways at once. A stale managed service is asked to retire over `SIGUSR2`, must durably acknowledge with matching identity before anything else happens, and is only then booted out; the replacement is bootstrapped after that. A retirement that is not acknowledged returns an error with the old service still running, rather than booting out on a hope.
- `startup.rs` reads `launchctl print`'s exit fields in the shapes launchd actually prints, checked against it rather than assumed: `last exit code = 0`, `(never exited)`, a bare number, a sysexits number annotated as `78: EX_CONFIG`, `last terminating signal = Segmentation fault: 11` (which replaces the exit code line rather than joining it), and `last exit reason = JETSAM_…`. A missing program is launchd's own `78: EX_CONFIG`, not the 126 or 127 a shell would report, so that is what names an unlaunchable runtime. A signal outranks an exit code, and a signal line that cannot be read resolves to unknown rather than falling back to a code it contradicts.
- Readiness stops waiting once three consecutive half-second checks agree the process exited and was not replaced, well inside launchd's five-second restart throttle, so a crash loop reports in about a second and a half instead of at the thirty-second deadline. The port, launchd and the clock reach it through `ServiceWatch`, so the timing is tested rather than asserted: a fake advances a virtual clock when the loop sleeps and records the instant of every `launchctl print`. That covers a slow-but-healthy start keeping its whole deadline, a crash loop giving up inside two seconds, a process returning between deaths restarting the count, and the subprocess staying spaced at the liveness interval rather than running every poll. Changing any of the three constants fails those tests. The panel gets one sentence, naming the cause when the server named one, and the exit status, log tail and readiness message go to the app's log. An exit line the host cannot read resolves to unknown, which never shortens the wait; the deadline is unchanged for a process that is alive.
- `src-tauri/src/gateway/infrastructure/macos/staging.rs` copies the bundled runtime to private immutable per-label/fingerprint directories, removes removable bundle-supplied extended attributes, syncs both cloned and byte-copied files, verifies full-tree parity with the packaging digest, and publishes atomically before service mutation. Existing versions are verified and retained; launchd arguments name the staged directory and no search path derives from it — the gateway addresses `nessa`, `node`, the ACP entry and `nessa-mcp` absolutely, so the staged directory is on neither the service's `PATH` nor the agent's. Tests cover cross-language Unicode/framing parity, private permissions, symlinks, rejected special entries, normalized file metadata, corrupt/existing versions and interrupted attempts.
- `src-tauri/src/gateway/infrastructure/macos/pruning.rs` collects the staged versions nothing can be running, under the same per-label lock, once the service has advertised its identity. `removable` is the whole rule and is pure: a published fingerprint directory goes only when it is neither the registered nor the running version and neither an unanswered retirement request nor an unacknowledged fence names it. `RetirementEvidence` carries `retired` for exactly that distinction: admission fencing needs only a recorded cause, while collection needs to know whether the old gateway finished and was booted out. An interrupted `.staging-` attempt goes because holding the lock means nobody is staging; every other name is left alone. `RuntimeVersions` is the directory seam, so the rule is tested without a filesystem and the real `LabelDirectory` revalidates each entry as a directory this user owns before removing it. No single entry can stop the pass: a refused removal, an entry the directory will not yield, and an entry with no valid text name are each reported and stepped over while the recognised versions beside them are still collected. Only a directory that cannot be listed at all ends the pass. What is reported is typed rather than a message string, so each line says what actually happened: a path appears only for an entry a removal was really attempted on, and a lossy rendering of an unusable name is never presented as somewhere to look. Failures are reported with their path and never reach registration's result. Reconciliations that return an error collect nothing, because the version they were replacing may still be running.
- `crates/nessa-server/src/desktop_runtime/` owns validated upgrade correlation, the admission-and-cleanup retirement use case, and private request/result/audit files. A managed old gateway stays alive until it has durably acknowledged retirement; launchd performs replacement only after that acknowledgement.
- `crates/nessa-server/src/composition/desktop.rs` bootstraps private local access and injects bundled provider paths.
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
that evidence forces a stale-service retry but never authorizes bootout. A pending
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
coordinates an opaque credential binding and its asynchronous storage port, `domain/value_objects/`
owns the rolling idle lifetime used to validate every stored session, and `adapters/`
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
`domain/value_objects/` owns `AgentId`, the host's three-way `HostAnswer`, and
the `Readiness` rule that turns two answers into one thing to tell the person;
`application/` owns the `AgentProbe` port, whose typed `ProbeFailure` keeps "not
signed in" apart from "could not tell", the `ReadAgentReadiness` use case that
only asks and maps, and `SharedAgentReadiness`, which bounds what one
unauthenticated request can cost: concurrent callers share a single in-flight
probe (never a cached answer) and stop waiting for it at a deadline;
`infrastructure/local.rs` asks this machine, with the
runtime root, API key and config directory resolved once in composition, while
`infrastructure/claude.rs` holds what is true of Claude Code alone — its
keychain item, its credentials file and what makes one a real sign-in, and the
environment variables the launcher passes through — so a second agent gets a
sibling module rather than a branch; `entrypoint/http.rs` owns the wire
vocabulary and the cross-origin rule for `GET /onboarding/agents`, and answers a
reading it could not obtain with 503 rather than an invented readiness.
Composition injects `LocalAgentProbe` through `ProductDependencies`, and the
handler receives the shared reader over it alone via `FromRef`. Tests under
`tests/agents/` split domain rules, application orchestration, the shared
reader's bounds, the HTTP boundary, the local probe's failure modes, and
Claude's own sign-in conventions.

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
name derived rather than copied from input and one lock ordering every change;
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
Holds live until their conversation closes; what expires on its own is an unused
ticket. Tests under `tests/attachments/` split domain rules, the service over
doubles, the real store on a real filesystem, audit records, the adapters, the
HTTP boundary, the wire mapping, the socket and router, and the facts this
context and the published protocol schema must agree on (`agreement.rs`).

## Command-line surface

The `nessa-server` crate builds the `nessa` executable. `cli/entrypoint/` parses
commands, `cli/application/` coordinates token requests through its gateway port,
and `cli/infrastructure/` implements the bounded local WebSocket adapter.
`composition/cli.rs` wires the adapter, clock, identities and stdout/stderr.
Tests mirror those responsibilities under `tests/cli/`; `scripts/smoke-auth.mjs`
checks actual process output and authenticated server effects. Offline bootstrap
remains in `composition/auth_command.rs`; it requires explicit `--local` selection.
Cloud auth is reserved but not implemented. See [local auth](guides/local-auth.md).
