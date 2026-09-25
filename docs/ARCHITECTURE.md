# Architecture

The map of this repository: what the pieces are, where a change goes, and what
must stay true. Read this before your first change. Update it in the same change
that invalidates it.

For extension and dependency rules, see [AGENTS.md](../AGENTS.md),
[coding standards](../CODING_STANDARDS.md), and
[codebase structure](codebase-structure.md).

Every change must meet the [repository-wide organization gate](../CODING_STANDARDS.md#organization-across-the-repository).
Keep ownership maps, tests, and documentation aligned with the implementation;
this applies equally to host, shell, server, SDK, and supporting scripts.

## The problem

Nessa is a floating chat panel. On macOS it is a menu bar extra: a transparent
surface summoned from the tray or a global shortcut, with no Dock icon and no
titlebar. On Linux it is the same panel, reachable from the system tray when
the desktop provides one and from the taskbar otherwise — there is no menu bar
to hang from, and a hidden window with no taskbar entry would be undiscoverable.

The two hard parts are **the window** — placing, sizing, and re-fitting a
chromeless panel across displays without the page's contents jittering during a
resize, with a frost that can be turned off — and **the conversation surface**
— a turn list driven by authenticated gateway conversation commands and bounded
replacement views from the SDK Agent.

## Code map

**Rust host** (`src-tauri/src/`) — everything that is the operating system's
opinion rather than the product's.

| File | Owns |
| --- | --- |
| `main.rs` | The entry point. Assembles `HostDependencies` in `setup`, wires the tray, shortcut, and window, and hands the bundle on. It never mentions macOS or Linux: OS behaviour is injected through `platform::current()`. |
| `launch.rs` | The desktop executable's empty command-line contract. It refuses the first supplied argument before Tauri or a platform window host is initialized. |
| `composition.rs` | The composition root: the one place the host's outside things are constructed — settings, shortcuts, the surface credential, the agent credential writer and audit, the independent `CredentialSaveTargets` authority derived from the durable namespace, the gateway, the release source — and the bundle every command and menu is given. Before correlation allocation, intent audit, or keychain effect, the save use case compares every field of the writer's claimed target with that canonical authority. Nothing below composition reaches back for a dependency. |
| `updater.rs` | Whether a newer Nessa is published and installing it. `ReleaseSource`, `CheckOutcome`, `Installer` and `Restarter` are its ports; the decisions are pure and tested, and the module header states which adapters are not. |
| `attachments/` | Choosing files to attach, and reading the ones that turn out to be images. Four ports, because they are four different outside things: `FilePicker` is the OS dialog, `ChosenFiles` is the filesystem (a chosen file's kind, length and bytes, which fail the same ways at the same moment), `AttachmentTickets` is the desk that mints and spends the one-shot tickets — the operating system's randomness and clock, and the port that carries the rule that a page cannot name a path — and `ContentTypes` is the platform's type database — Launch Services on macOS, shared-mime-info on Linux, nothing elsewhere — so a `.ico` or `.svgz` is recognised as an image without this app keeping a list of formats. That answer goes where a dropped file's `type` goes, which is what keeps one file from taking two routes. Anything that is not a regular file is refused before it is opened, and both the look and the read have deadlines on their own threads, so a FIFO or a stalled mount cannot wedge the panel. A read is authorised by a one-shot ticket the picker minted, never by a path the page names. The page calls `choose_attachment_files` and `read_attachment_bytes`; the host calls the dialog plugin, so `capabilities/` grants the webview nothing. |
| `surface_credential.rs` | The bundled panel's token: where it lives for a stage, and `CredentialRefusal` for why there is not one. Only the bundled window may ask. |
| `local_data.rs` | The stage-scoped data root this process reads, mirroring the server's own path rules. |
| `stage_port.rs` | The loopback port the gateway registers for a stage, from `protocol/defaults/gateway-ports.json`. macOS-only, like the registration that reads it. |
| `gateway/domain/`, `gateway/application/`, `gateway/infrastructure/` | One retryable background-service startup owner and its native launchd adapters, injected from `main.rs`. The application publishes revisioned starting, ready, and failed snapshots to bundled surfaces; independent credential loads reconcile the complete service again while concurrent callers share one attempt. Domain evidence validates each request cause and initiator, attempt correlation, target, before/after incarnation, and the ordered lifecycle journal from intent through plans, command results, observations, and outcome. One stage-locked journal session validates live and restored records and acknowledges delivery only after retained-directory synchronization, binding and file-identity checks, and strict read-back. It settles an unfinished intent only after fresh state proves no effect was planned, completes missing observations and outcomes when fresh native state agrees, and otherwise keeps the exact attempt unresolved. Automatic quit uses the same journal and one absolute deadline with an application-owned proof-to-dispatch claim. Physical service results and audit delivery remain separate facts. The adapter verifies the running runtime fingerprint and owns acknowledged update replacement; gateway lifetime remains independent of the desktop. The domain also holds `SearchPath`, the validated `PATH` value; `LoginShellPath` is the port behind which the account's own login shell is read once per host process for the path the agent will be given. |
| `links.rs` | Where a clicked link goes. A pure `decide` allows the app's own origins (`tauri://localhost`, `http://tauri.localhost`, and the dev server in a `tauri dev` build alone), hands `http`, `https` and `mailto` to the OS, and refuses everything else — the panel has no address bar to come back from, and its webview is the one the host's commands are granted to. Applied by a Tauri plugin, because the panel window is declared in `tauri.conf.json`. The module header lists which ways out of a page the navigation policy does not see. |
| `host.rs` | The host/shell seam: event names and the `PanelSize` payload. The frontend lists the same names in `src/host/window.ts`; a test fails if they drift. |
| `panel.rs` | The panel frame: opening size, lower-right placement, show/hide. The tray and the shortcut request a toggle; they do not fit the frame. |
| `tray.rs` | The menu bar extra (macOS) or StatusNotifierItem (Linux), and the surface-toggle request. Creating it is survivable: a desktop with no tray still launches. |
| `shortcut.rs` | Registers / re-registers the global `panel.summon` accelerator from the shortcuts cache. |
| `shortcuts.rs` | Stage-scoped `shortcuts.json` cache: seed from bundled protocol defaults. |
| `settings.rs`, `settings/storage.rs` | The on-disk settings shape (panel geometry) and its defaults, over a `Storage` port that `shortcuts.rs` reads through too. Summon is not here — see `shortcuts.rs`. |
| `platform/` | The OS host. `Host` is the contract — window shaping, and `open_externally` for a link leaving the app; `current()` injects one implementation for the compiled target. Commands `set_frosted` and `panel_size` live here too. |
| `platform/macos/` | Accessory app, `/usr/bin/open` for links, `NSVisualEffectView` frost, WKWebView pin, AppKit live-resize notifications. The panel stays open when focus moves to another app and joins all desktop Spaces, with fullscreen auxiliary behavior enabled. |
| `platform/linux/` | WebKit DMA-BUF prep, `xdg-open` for links, GtkFixed pin, CSS frost (no-op natively), allocate-based live resize, shown on the taskbar at launch. |
| `platform/other/` | Webview fills the window; size events only. |

**Launch** ([justfile](../justfile)) — `just server` / `just dev` / `just web` / `just release [stage]` / `just release [stage] fast`. The desktop launch tooling resolves one stage for Vite and the host, and packaged frontend assets record that stage for the Rust build to verify before embedding them. Bundle names and Linux WebKit/GTK checks live in the justfile, not a second host layer. Windows recipes are written, not yet run on a Windows box.

**React shell** (`src/`) — everything that is on screen.

| Path | Owns |
| --- | --- |
| `main.tsx`, `store.ts` | Composition root. Mounts the panel, the session lifecycle, and product projections. |
| `conversation/` | The conversation vertical. See the table below. |
| `session/` | Authenticated wire session to `nessa-server` via `@nessa/client`, including verified local endpoint discovery, health, and reconnect lifecycle. |
| `panel/` | The floating-window chrome. See the table below. |
| `host/` | Injected host features and the window seam (`window.ts`). |

**Conversation vertical** (`src/conversation/`) — one feature, independently testable.

| Path | Owns |
| --- | --- |
| `model/` | Shared language: `Conversation`, `Turn`, `ConversationTabs` (`conversations` + `activeId`), and `CommandFailure` — why a conversation command did not do what was asked, in the panel's words rather than the gateway's. Discriminated turns and phases. No id mill, and no client SDK: the model states product rules in its own terms, and `adapters/gateway/` is where they meet the wire's. |
| `application/local-tabs.ts` | UI-session store shape: the shared tabs plus local id counters. UI-local turn counters; durable conversation and submission UUIDs remain separate identities. |
| `application/usecases/` | One file per command. Local drafts and tabs, send/steer/queue, stop, permission replies, and replacement-view application. |
| `application/ports.ts` | `ConversationGateway` for local draft and tab operations; `ConversationEffects` for what the panel may ask the product to do, including staging an image's bytes; the typed `AttachmentStagingError`, `SubmissionRefusedError` (a message the gateway did not take, so the draft comes back), and `ControlFailedError` (a control it answered with a reason, a `ControlOutcome`, or both — the reason may be absent while the outcome is certain). |
| `adapters/gateway/local.ts` | In-process draft/tab projection. Remote effects live in `adapters/gateway/effects.ts` and use the shared authenticated client; staging is begin, then upload of the original bytes only when a ticket was issued; it answers with the reference the gateway stored, and maps the client's failure codes to the panel's typed reasons. |
| `adapters/store/` | Redux projection and command thunks. Thunks invoke injected effects; reducers apply local UI state and returned views. |
| `ui/` | Transcript, thinking pill, `useConversation`. Paints and dispatches. `message-images.tsx` paints a sent turn's images: the local preview when this window has one, a labelled placeholder when only a reference is known. |
| `model/attachments.ts` | File parts with their upload state (a stored file carries the gateway's whole returned reference), reference-only image parts, the preview budgets, and the message rules counted over stored references (10 images, 10 MiB together). No per-image byte or pixel limit lives here: that is the gateway's, per model. `messageImages` decides which parts of a message go as image references, or the one reason none can, and `messageFiles` the paths it points the agent at. Which of the two a file takes is `linkedFile`: an image is carried, and anything else the host named a path for is pointed at. A file with neither — a browser gave the bytes and nothing could say where they came from — cannot be sent, and the byte budgets are counted over held bytes alone, so a video attached by path is bound only by how many files a draft shows. `declaredMediaType` names a file the browser gave no type (camera RAW, some HEIC) by its extension, and `previewableImage` says which images a webview can paint. |
| `application/usecases/attachments.ts` | Attach to the originating conversation, remove individual draft files, and own every step of a draft file's upload state; a result for a removed file changes nothing. |
| `application/usecases/release-uploads.ts` | Forget a draft's stored images once the gateway conversation that held them has been closed, and bound how many already-sent originals are kept to paint the transcript. |
| `application/usecases/upload-failure.ts` | Why an upload failed, in words, and whether a retry could end differently. One owner for both, so the tile and the composer's notice cannot drift apart; exported through the barrel for the panel. |
| `application/usecases/send-draft.ts` | Every local reason a draft is declined, as a pure `declineReason` the store shows and rejects in one place; what a refusal says; and how a submission's outcome lands on its turn. |
| `application/usecases/control-failure.ts` | What a failed conversation control says. The client has one constant for every control — "did not return a trustworthy acknowledgement" — so this speaks wherever that would be false and stays silent wherever it is still true. Which of the three sentences is shown is decided by the `ControlOutcome` (refused, applied, unknown), never by the reason, which only shapes it; a review the gateway left pending is refused under codes this build has no word for. |
| `testing.ts` | What another context's tests may import instead of reaching into `adapters/` or mocking the barrel with a copy of a rule. No component, and not for product code. `src/session/testing.ts` is the session's. |
| `model/identity.ts` | The agent's name, seed, and hue wheel. |

**Session vertical** (`src/session/`) — WebSocket control-plane connection.

| Path | Owns |
| --- | --- |
| `model/` | `SessionPhase`, status copy for the empty state. |
| `adapters/client/` | `connectDevSession` (injected native endpoint and credential loading, authenticated session, health; closes on probe failure) + injected session handle (live client outside Redux). |
| `adapters/store/` | Redux projection of connection status (`hello` / `health` only). |
| `adapters/lifecycle/` | React lifetime plus `supervisor.ts`: fresh connections after typed transient startup failures or exhausted SDK retries, capped backoff, stale callback disposal, explicit retry for terminal errors. No message replay. |
| `ui/use-session.ts` | Hook the panel reads for status. |

Chat adapters receive the composition-owned session handle; they do not open another socket or put `NessaClient` in Redux. The panel uses `AgentNotification` above the pill for connection recovery and explicit admission retry. Transport recovery only refreshes the conversation; uncertain message receipts retain their submission identities.

**Panel vertical** (`src/panel/`) — the floating window, not the product.

| Path | Owns |
| --- | --- |
| `model/` | `Surface` — frosted or clear. |
| `adapters/` | Host subscriptions: colour scheme, edge reveal, panel frame, frost, remembered surface, compositor flush, config-driven tab shortcuts. |
| `ui/app.tsx` | The chrome: stage, glow, resize handle, tab strip, composer. Renders; no effects. |
| `adapters/attachment-resources.ts`, `adapters/dropped-image.ts`, `adapters/dropped-text.ts` | Bounded object-URL resources that also hold each file's original bytes for upload and stop counting a file once its message has been taken, remote image reads, and external drop representations. `addChosen` is the other way in: a file the host's picker named by path, which this window holds nothing of and which spends none of the budget. |
| `application/upload-image.ts` | The order of one upload — hash the original, then stage it, checking after the wait that the tile is still there — and which waiting images start next (three in flight per window). No image processing, which is the gateway's. |
| `application/attachment-notice.ts`, `ui/attachment-notices.tsx` | Everything the composer says about attachments. One typed refusal per way the panel turns something away, and two subjects that are never ranked against each other: what the draft is holding, and what was just turned away. Both render when both are true, the refusal nearest the composer; one action at most, being the uploads worth retrying or the file picker where choosing again could end differently. The words for a failed upload are the conversation's. Rendered through `AgentNotification`, like every other notice in this pane; no notice is red text beside the composer, and the reading status is a live region mounted before there is anything to read. |
| `ui/attachment-drop-zone.tsx` | The drop zone's bounds and both halves of a drop: what passed, and one refusal naming the rule that refused the rest. A component rather than props on the chrome, so the wiring the report's oversized file travels through is exercised by a test. |
| `adapters/sha256.ts` | The SHA-256 that identifies an upload to the gateway. It reads Web Crypto, so composition injects it and tests substitute their own. |
| `ui/use-attachment-uploads.ts`, `ui/attachment-tile.tsx` | Start an upload for every draft image that has not had one; paint a tile's upload state with its retry. |
| `adapters/use-drop-navigation-guard.ts` | Prevent dropped URLs from navigating the webview. |
| `application/link-notice.ts`, `adapters/use-link-notice.ts` | The sentence for a link the host did not open, and the subscription that carries it. The decision is the host's, in `src-tauri/src/links.rs`; this is what the person reads when a click went nowhere. |
| `ui/use-file-attachments.ts` | Remote pending previews, originating conversation, viewer state, and the last refusal — a typed reason, never a sentence, kept with the conversation it was said to. It is put down by the things that answer it, named as calls rather than worked out from the draft afterwards: files actually attached, a file removed, and `useComposer` reporting a draft that has gone. Local files use synchronous object URLs. Uploading is not its job. `chooseFiles` asks the host's picker for paths and falls back to the page's own file input when there is no host; `pickerRefusal` turns what the host would not hand over into one of this composer's typed reasons, so a selection is never quietly one file short. |
| `adapters/dropped-folder.ts`, `ui/use-folder-drop.ts` | Bounded sequential folder traversal, cancellation, originating draft and pending-send guard. |
| `ui/use-content-drop.ts`, `ui/use-attachment-menu.ts` | Drop acceptance/routing and menu geometry lifecycle, separate from rendering. |
| `ui/attachment-preview.tsx`, `ui/attachment-icon.tsx`, `ui/add-attachment-menu.tsx` | Lazy shared file preview, file-kind icons, and composer Add menu. |
| `ui/waveform-icon.tsx` | The voice glyph in the composer. |

The composition root injects an attachment resource store into the panel. Redux
keeps metadata, URLs, and each file's upload state; its subscription reconciles resource IDs after commands and revokes URLs
that are in no draft and no sent turn, including closed conversations. A sent
turn keeps its previews so the transcript can paint them and so a refused send
can return them to the draft. The resource store shares the product store
lifetime so React remounts do not invalidate previews. See
[images in a message](guides/gateway-chat.md#images-in-a-message-panel-and-client).

## Boundaries

Three, and they are all real:

**Host ↔ shell.** They talk over exactly one seam (`src/host/window.ts` on one side,
`host.rs` plus the command handlers on the other). Everything crossing it is an
explicit command or event, never shared state. This is what lets `pnpm dev`
open the UI in a browser with no host at all.

**Product store ↔ host subscriptions.** Conversation state is Redux, so an
agent can dispatch it. Window, pointer, frost, and clocks stay in hooks.
They do not belong in the store, and the store does not import the chrome.

**Owner of a preference.** State has one owner and one direction. The surface
choice is owned by the frontend and reflected into the tray's check mark; the
tray requests, it does not decide. New preferences point the same way, or they
get an ADR explaining why not.

**Desktop host ↔ gateway runtime.** The prepared runtime tree has one fingerprint.
Under its service-label lock, the host copies and verifies the bundle into a
private version directory outside the app before any service mutation. Exclusive
atomic publication prevents replacing an existing version; launchd arguments name
only the staged directory, and no search path derives from it — the service runs
everything in that directory by absolute path, so neither the gateway's `PATH`
nor the agent's contains it. A host process fully validates a published version
before first use, then caches only that exact path, fingerprint, and private root
directory identity; later reconciliation still checks the complete definition and
live process identity without hashing the whole runtime again. Published versions
are retained and never repaired or garbage-collected automatically.
The desktop compares it with health's fingerprint, persisted service generation,
and canonical runtime-instance UUID, requiring the advertised process ID to match
the exact launchd service PID. A generation is reused only for the same complete
on-disk definition while unfenced. Changed or retired definitions receive a fresh
random generation, so configuration reverts cannot reuse a retired identity.
A matching recorded retirement cause forces reconciliation even when health
otherwise matches or cleanup failed; failed bootstrap retries retain an unfenced already-published generation.
A published request targeting the live instance/generation also forces retry when
its result is missing. Requested and actual result generations remain separate;
a rejection without a retirement cause cannot fence an unrelated generation. A
restored fence reuses its original validated principal, lifecycle cause, and
correlation instead of reconstructing attribution from the later upgrade attempt.
Unknown launchctl output fails closed; legacy headerless health additionally
requires a sole matching listening PID.
For a managed update, `crates/nessa-server/src/desktop_runtime/` validates the
correlated request, closes conversation admission, joins admitted commands,
attempts owner cleanup, and persists upgrade audit evidence. Only a successful
private result, correlated to that runtime instance and synced with its directory,
authorizes the host to unload the old service. A failed result,
an unavailable loaded service, or a foreign listener preserves the process.
Inactive PID-less registrations are preserved too: an on-disk definition cannot
prove launchd's independently retained program, arguments and environment.
This one-time recovery limitation concerns direct-app registrations already broken
before migration. Once staged, app replacement leaves the registered runtime
intact so its normal managed retirement boundary remains available.
Before bootstrap, the host durably records the exact service, definition, runtime
fingerprint and generation under the service-label lock; it retains that record
until readiness succeeds. A retry may unload an unambiguously PID-less unavailable registration only when that
host-owned record, the desired definition and the complete on-disk definition all
agree. Missing, malformed or contradictory evidence preserves the service. This
lets a failed readiness check retry the host's own new registration without using
the plist alone as authority. A failed bootstrap clears the record only when
launchd positively reports the label unloaded. Restoring an old plist is not a
safe rollback.

## Invariants

Things that must stay true. They are invisible in the code, which is why they
are written here.

- `main.rs` is the app composition root. OS-specific hosts are injected by
  `platform::current()` — one `Host` for the compiled target. Shared modules
  never name macOS or Linux.
- The panel frame is reapplied on every show. Nothing may cache a frame across
  shows — that is the bug this design exists to prevent.
- The page's viewport does not move during a resize. The webview is pinned; the
  window moves over it; the shell is told the window's size.
- The window seam is guarded. Any new host call goes through `src/host/window.ts`
  and no-ops outside Tauri, or the browser workflow breaks silently.
- A settings file missing keys still launches. Any new key has a default.
- Gateway readiness means the expected prepared-runtime fingerprint answered on
  health with the expected service generation, runtime-instance UUID and PID
  matching the exact launchd service.
  A generic HTTP 200 is insufficient.
- Packaged gateway startup has one host owner. It begins independently of either
  webview, exposes a revisioned snapshot plus bundled-window events, and serializes
  retries with credential-load reconciliation. Event callbacks may race the owner,
  so consumers use revisions to discard stale delivery. A successful repeated
  validation of the same readiness identity keeps the current ready revision;
  a changed confirmed identity, a failure, and recovery from a failure advance it.
  Attempt identifiers are allocated before lifecycle admission, and the lifecycle
  lock alone selects the running receipt and its one pending successor. The receipt
  owner is launched independently of the admitting caller.
- launchd restarts the gateway when its process ended unsuccessfully, and only
  then. A failure that starting again cannot fix exits zero on purpose — the one
  status launchd reads as "do not start me again" — but only after the reason it
  could not carry is durably recorded beside its log, because that record is the
  only thing that authorizes the host's one fresh attempt. A record that could
  not be published keeps its non-zero exit and its restarts. Crashes and failures
  that can clear keep theirs too, and so does a managed gateway that merely
  served and was asked to stop: being signalled is not being told to stay
  stopped, and `launchctl bootout` — which unloads the job first — is how that
  service is ended. Only the launch the host registered exits that way, or may
  publish a record or forget one of its own generation; a server nobody
  registered keeps the shared table's exit codes, exits zero when it is stopped,
  and touches neither. The gateway log is bounded at every start, with one
  previous file.
- Gateway updates serialize by launchd service identity. Managed replacement
  requires correlated cleanup and audit acknowledgement; failed retirement never
  authorizes bootout. The pre-protocol gateway has one explicit legacy path.
- Installed agent runtime launches carry an inseparable executable and use
  authority. Every managed spawn records one bounded durable generation before
  process creation; only confirmed no-spawn or process-tree cleanup records its
  release. Superseded artifact removal holds the publication lock, rechecks the
  current artifact, takes the artifact lock without waiting, and requires a
  complete released-generation inventory. Replacement delivery remains unsettled
  until its exact cleanup obligation is durable, and a later replacement requires
  the exact retained settlement receipt.
- Failures at the edges — blur, sizing, tray, viewport — are reported and
  survivable, not fatal. The panel opening unblurred, or without a tray, beats
  the panel not opening.
- Linux is a first-class host. The panel is reachable without a menu bar: the
  taskbar is on, the window opens on launch, and close quits if there is no tray.
- On Linux the webview's grandparent is the GtkWindow. An extra widget in
  between panics inside a GTK callback and aborts the process.
- A component either renders or coordinates, never both.
- There is no `utils` module, on either side.
- Product state an agent must drive lives in the Redux store. Host, DOM, and clocks stay in adapters. See [adr/0001-redux-toolkit-for-product-state.md](adr/done/0001-redux-toolkit-for-product-state.md) and [adr/0003-panel-vertical.md](adr/done/0003-panel-vertical.md).

## Cross-cutting

- **Failure policy:** edge failures degrade the surface, they do not stop the
  launch. Log with the `[nessa]` prefix and continue.
- **Platform code** lives in `platform/{macos,linux,other}` (Rust) and
  `src/host/{macos,linux,browser,other}.ts` (shell), injected through `Host` /
  `HostFeatures`. Shared modules do not contain `cfg(target_os)` or
  `hostKind ===` branches.
- **Nothing in the shell reaches the OS directly.** It goes through the seam.
- **Frost:** native `NSVisualEffectView` on macOS; CSS `backdrop-filter` on
  Linux and in the browser. The macOS CSS path is the one that smears.

## Where to make a change

| You want to change | Go to |
| --- | --- |
| Where or how big the panel appears | `panel.rs` (`frame_on`, `apply_configured_size`) |
| What the tray menu offers | `tray.rs`, plus the frontend if it owns the state |
| The summon shortcut | `settings.rs` for the key, `shortcut.rs` for registration |
| A new persisted preference | `settings.rs` (with a default), then its owner |
| A new host event or payload | `host.rs` and `src/host/window.ts` together |
| Leftover transcript tiles on a layout compositor | `flush_compositor` in `platform/` plus `useFlushOnTurn` in `src/panel/adapters/` |
| The conversation surface (tabs, drafts, empty transcript) | `src/conversation/application/usecases/` (commands), `adapters/gateway/` (local session), `adapters/store/` (projection) |
| The transcript chrome | `src/conversation/ui/` |
| The panel chrome or composer | `src/panel/ui/app.tsx` |
| Resize jitter, the pinned webview | `platform/{macos,linux,other}/viewport.rs` |
| Native frost | `platform/macos/vibrancy.rs` |
| The border glow | `src/panel/adapters/edge-reveal.ts`, and `platform/*/live_resize.rs` |
| OS-specific window behaviour | `platform/` — add a method on `Host`, implement it in the OS folder |
| Shell behaviour that differs per OS | `src/host/` — add a field on `HostFeatures`, set it on each host |
| Anything that talks to the host | `src/host/window.ts` — and only there |
| Execution/session, tool, permission, and scheduling invariants | `crates/nessa-sdk/src/domain/agent_execution/`; see [the domain map](../crates/nessa-sdk/src/domain/agent_execution/mod.rs) |
| Agent invocation, session snapshots, queueing, and hooks | `crates/nessa-sdk/src/application/agent_execution/`; see the [SDK guides](../crates/nessa-sdk/docs/agent_execution/README.md) |

## Execution domain foundation

The [SDK execution domain](../crates/nessa-sdk/src/domain/agent_execution/mod.rs)
owns execution/session identities, immutable prompts and tool values, tool and
permission entities, and a live session consistency boundary. Tools and reviews
belong to that session; scheduling value objects describe admitted invocation
order and lifecycle evidence. Domain code performs no provider, storage, or clock
effects. `InvocationHistory` validates agreement between delivery intent,
scheduling, observations, and local settlement for both recording and restoration.
See the [repository map](codebase-structure.md#agent-sdk-foundation) and
[domain tests](../crates/nessa-sdk/tests/domain/agent_execution/mod.rs).

Application orchestration and infrastructure effects use these domain rules;
their implemented contracts are described below.

## Agent entry point and local sessions

The [Rust SDK](../crates/nessa-sdk/README.md) exposes `Agent` as the public
invocation/control entry point. It combines an injected `AgentProvider`, typed
hooks, and a `SessionManager` with an exclusive storage lease. In-memory and private
file adapters retain local snapshots. Model capabilities constrain input admission;
negotiated operation capabilities describe native steering and context restoration.

Agent owns sequential queueing, priority boundary steering, native injection,
withdrawal, and idempotent submission recovery. The domain protects execution,
tool, and permission invariants; adapters own provider translation and effects.
Mandatory permission audit remains separate from session snapshots.
The host supplies verified attribution and authorizes commands before SDK access.

Behavior belongs in the [SDK guides](../crates/nessa-sdk/docs/agent_execution/README.md),
especially [Agent/storage](../crates/nessa-sdk/docs/agent_execution/agent.md),
[scheduling/retries](../crates/nessa-sdk/docs/agent_execution/scheduling.md), and
[permissions](../crates/nessa-sdk/docs/agent_execution/permissions.md). The
[structure guide](codebase-structure.md#agent-sdk-foundation) maps owning modules.
The gateway now owns a prepared shared Agent for each authorized conversation. The
conversation context owns durable creator/organization metadata and a bounded
read projection; SDK Agent remains the sole scheduler and execution authority.
The projection also carries ordered runtime-owned declined-review notices from
persisted invocation events, replacing selection with later local write evidence
by identity without turning it into permission authority or provider output.
NessaClient sends stable-ID commands over its existing authenticated socket.
The floating panel polls current replacement views, displays streaming output and
permission choices, distinguishes provider startup from model thinking, and queues
follow-ups while attachment is pending. Create, read, and queue admission use the
ordinary client command deadline; provider startup settles later through the same
replacement view and durable SDK receipt. Closing a tab detaches a view;
Stop explicitly closes active and queued work.

See [gateway chat](guides/gateway-chat.md) for configuration, ownership, commands,
limits, and durability. The server stores SDK JSONL snapshots at consequential
boundaries and mandatory audit records independently. Unfinished streaming text
can be lost on crash. Reads are bounded current views, not a durable cursor stream.
No second event database is required for this initial integration.

A message refers to an image by digest, media type and size; its bytes never
ride the product socket. `attachment.begin` on the authenticated socket answers
with a single-use, five-minute ticket bound to one file, conversation and caller,
and `PUT /attachments` streams the bytes under it. The gateway verifies the
transfer against the ticket, normalizes an image through an injected port, stores
bytes once per digest, and records that the conversation holds them. The
conversation service accepts only references its conversation holds; the ACP
adapter reads bytes by content through the SDK's image port, and its frame bound
is derived from the message's image budget. Closing a conversation releases its
holds, with audit evidence for every transition. See the
[attachments module map](../crates/nessa-server/src/attachments/mod.rs).

What happens to an attached file is decided by its type and never by the
gesture that attached it: an image is uploaded and normalised wherever it came
from, and anything else is named. That rule is `declaredMediaType`, and it is
why a file chosen through the host's picker — which reads nothing, so its name
is the only evidence — has its images read back through `read_attachment_bytes`
before they are uploaded.

A file that is not an image does not travel at all. The gateway, the agent and
the panel are on one machine, so the message names an absolute path and the
agent opens it itself — as a `resource_link` block, which the adapter writes into
the prompt as a link and which the model may then read with its own file tool, or
not. The gateway checks only what makes a path mean the file that was chosen
(absolute, no control character, every component below the root a name) and
deliberately does not ask whether the file is there, what it is, or whether it
is inside the workspace. It checks nothing about markdown: the two strings the
adapter interpolates into that link are constrained where they are built — the
URI percent-encoded to an allowlist, the label backslash-escaped over ASCII
punctuation — rather than by constraining what a person may call a file, after
three successive attempts to name the dangerous characters each missed one. The
agent's own `Read` is in permission `ask`, so the person approves each read and
sees the path they are approving. A message naming files is recorded before it is
admitted, with the paths, the verified caller and the submission — evidence of
the naming, which is intent, and never of a read or even of an admission. An
audit sink that cannot take that record refuses the send. This is local-only
by construction. See [ADR 0013](adr/done/0013-files-by-path-not-by-payload.md).

ADRs 0009 and 0011's exact replay and broader collaboration remain proposed work.
Remote TLS/device provisioning, audio and video in a message, and more
provider adapters remain separate features. Existing design proposals do not replace the implemented Agent contract.

**Identity/access contracts** (`crates/nessa-auth`) — reusable library, no binary.
Owns domain identities/memberships/credential metadata, boundary DTO validation,
and injected session authentication contracts. Embedded Cedar evaluates product policies through the application port. The local credential backend and guarded `/session` gateway are implemented.
See [local authentication](adr/done/0010-local-authentication.md) for setup and current limits. See the [crate guide](../crates/nessa-auth/README.md).

**Local agent credential values** (`crates/nessa-agent-credentials`) — pure
shared domain, no binary or effects. The gateway adapter constructs its
validated private credential and stage/instance namespace through a
caller-owned read port. The canonical keychain names
are infrastructure data in `protocol/defaults/agent-credentials.json`; provider
environment mapping stays in the gateway adapter. Desktop composition writes
validated API keys to the matching login-keychain item through a host-owned port
and records correlated, secret-free intent and outcome evidence. Gateway
composition injects one credential source into readiness, Claude process launch,
and packaged OpenCode cold-open resolution. A standalone explicit OpenCode
runtime instead uses only `OPENCODE_API_KEY` captured when composition starts;
it does not read the stage-scoped store or promise live environment refresh. See the
[crate guide](../crates/nessa-agent-credentials/README.md).

## Gateway authorization

The running gateway mounts one authenticated product flow on `/session`. Each request checks current credential validity and membership;
product operations additionally require embedded Cedar approval for the action
on the server-resolved gateway resource at operation admission. Admitted handlers
and responses may finish after revocation; later operations read the latest
committed state. Only auth mutations serialize; network writes share no admission mutex. The HTTP `/health` probe returns
only liveness, without product state.

The panel authenticates using its distinct private surface credential loaded by
the native host. The SDK supports injected credential storage and a Node file
source. Composition loads namespace `config.json` and injects registry limits
and session deadlines. Use the
[local SDK/CLI guide](guides/local-auth.md) for gateway access and the
[adversarial review](reviews/local-auth-gateway.md) for validation and limits.

## Nessa-owned tools over MCP

`crates/nessa-mcp` is the stdio MCP server for all Nessa-provided tools. Claude's
native file/web tools remain provider-owned. The gateway composes trusted MCP
server configurations into ACP; no tool request selects executable configuration.
The shell tool coordinates an injected Shepherd runner and private process audit
through its application ports. See the [MCP server](../crates/nessa-mcp/README.md).

## CLI surface

The `nessa` executable runs the gateway with `nessa server`. That command serves
only what was already provisioned; `nessa server --provision-local` additionally
creates the namespace's owner and panel credentials when they are absent, which
is what the desktop app and the `just start` / `just server` developer loop ask
for. Provisioning is a guard, never a rotation. Online `auth token`
and `doctor` commands use the existing authenticated product protocol as the CLI
surface; they do not access the server registry. Offline `auth init --local`
bootstraps first access, and local recovery/provisioning retain their exclusive
registry lock. Cloud selection fails explicitly until its implementation exists.
See the [CLI module map](../crates/nessa-server/src/cli/mod.rs) and
[local auth guide](guides/local-auth.md).
