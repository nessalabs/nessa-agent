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
| `main.rs` | The composition root. Builds the app, wires the tray, shortcut, and window. It never mentions macOS or Linux: OS behaviour is injected through `platform::current()`. |
| `host.rs` | The host/shell seam: event names and the `PanelSize` payload. The frontend lists the same names in `src/host/window.ts`; a test fails if they drift. |
| `panel.rs` | The panel frame: opening size, lower-right placement, show/hide. The tray and the shortcut request a toggle; they do not fit the frame. |
| `tray.rs` | The menu bar extra (macOS) or StatusNotifierItem (Linux), and the surface-toggle request. Creating it is survivable: a desktop with no tray still launches. |
| `shortcut.rs` | Registers / re-registers the global `panel.summon` accelerator from the shortcuts cache. |
| `shortcuts.rs` | Stage-scoped `shortcuts.json` cache: seed from bundled protocol defaults. |
| `settings.rs` | The on-disk settings shape (panel geometry) and its defaults. Summon is not here — see `shortcuts.rs`. |
| `platform/` | The OS host. `Host` is the contract; `current()` injects one implementation for the compiled target. Commands `set_frosted` and `panel_size` live here too. |
| `platform/macos/` | Accessory app, `NSVisualEffectView` frost, WKWebView pin, AppKit live-resize notifications. The panel stays open when focus moves to another app and joins all desktop Spaces, with fullscreen auxiliary behavior enabled. |
| `platform/linux/` | WebKit DMA-BUF prep, GtkFixed pin, CSS frost (no-op natively), allocate-based live resize, shown on the taskbar at launch. |
| `platform/other/` | Webview fills the window; size events only. |

**Launch** ([justfile](../justfile)) — `just server` / `just dev` / `just web` / `just release fast` / `just release`. Bundle names and Linux WebKit/GTK checks live in the justfile, not a second host layer. Windows recipes are written, not yet run on a Windows box.

**React shell** (`src/`) — everything that is on screen.

| Path | Owns |
| --- | --- |
| `main.tsx`, `store.ts` | Composition root. Mounts the panel, the session lifecycle, and product projections. |
| `conversation/` | The conversation vertical. See the table below. |
| `session/` | Wire session to `nessa-server` via `@nessa/client` (S1: connect + health + dev-only ping). |
| `panel/` | The floating-window chrome. See the table below. |
| `host/` | Injected host features and the window seam (`window.ts`). |

**Conversation vertical** (`src/conversation/`) — one feature, independently testable.

| Path | Owns |
| --- | --- |
| `model/` | Shared language: `Conversation`, `Turn`, `ConversationTabs` (`conversations` + `activeId`). Discriminated turns and phases. No id mill. |
| `application/local-tabs.ts` | UI-session store shape: the shared tabs plus local id counters. A remote gateway mints its own ids and this type goes away. |
| `application/usecases/` | One file per command. The desktop UI currently owns local draft, tab, open, and close behavior; its send/stop paths do not invoke the gateway. |
| `application/ports.ts` | `ConversationGateway` — what the panel may ask the product to do. |
| `adapters/gateway/local.ts` | In-process UI-session gateway. The authenticated remote API is exposed separately by `@nessa/client`. |
| `adapters/store/` | Redux projection. Reducers call the gateway; they do not contain rules. |
| `ui/` | Transcript, thinking pill, `useConversation`. Paints and dispatches. |
| `model/attachments.ts` | Local file parts and per-file/draft budgets; file-bearing drafts cannot enter text-only sends. |
| `application/usecases/attachments.ts` | Attach to the originating conversation and remove individual draft files. |
| `model/identity.ts` | The agent's name, seed, and hue wheel. |

**Session vertical** (`src/session/`) — WebSocket control-plane connection.

| Path | Owns |
| --- | --- |
| `model/` | `SessionPhase`, status copy for the empty state. |
| `adapters/client/` | `connectDevSession` (native credential loading, authenticated session, health; closes on probe failure) + injected session handle (live client outside Redux). |
| `adapters/store/` | Redux projection of connection status (`hello` / `health` only). |
| `adapters/lifecycle/` | Mount/reconnect effect, owned by the composition root. Subscribes `onClose` before publishing ready. |
| `ui/use-session.ts` | Hook the panel reads for status. |

Chat adapters must use `getSessionClient()` from the session barrel — do not open a second socket, and do not put `NessaClient` in Redux.

**Panel vertical** (`src/panel/`) — the floating window, not the product.

| Path | Owns |
| --- | --- |
| `model/` | `Surface` — frosted or clear. |
| `adapters/` | Host subscriptions: colour scheme, edge reveal, panel frame, frost, remembered surface, compositor flush, config-driven tab shortcuts. |
| `ui/app.tsx` | The chrome: stage, glow, resize handle, tab strip, composer. Renders; no effects. |
| `adapters/attachment-resources.ts`, `adapters/dropped-image.ts`, `adapters/dropped-text.ts` | Bounded object-URL resources, remote image reads, and external drop representations. |
| `adapters/use-drop-navigation-guard.ts` | Prevent dropped URLs from navigating the webview. |
| `ui/use-file-attachments.ts` | Remote pending previews, originating conversation, and viewer state. Local files use synchronous object URLs. |
| `adapters/dropped-folder.ts`, `ui/use-folder-drop.ts` | Bounded sequential folder traversal, cancellation, originating draft and pending-send guard. |
| `ui/use-content-drop.ts`, `ui/use-attachment-menu.ts` | Drop acceptance/routing and menu geometry lifecycle, separate from rendering. |
| `ui/attachment-preview.tsx`, `ui/attachment-icon.tsx`, `ui/add-attachment-menu.tsx` | Lazy shared file preview, file-kind icons, and composer Add menu. |
| `ui/waveform-icon.tsx` | The voice glyph in the composer. |

The composition root injects an attachment resource store into the panel. Redux
keeps metadata and URLs; its subscription reconciles resource IDs after commands
and revokes URLs removed from drafts, including closed conversations. The resource
store shares the product store lifetime so React remounts do not invalidate previews.

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
The gateway owns one shared Agent for each authorized conversation. The
conversation context owns durable creator/organization metadata, mandatory
creation audit, and a bounded read projection; SDK Agent remains the sole
scheduler and execution authority. NessaClient sends stable-ID commands over its
existing authenticated socket. Closing a client view does not implicitly stop
work; the explicit close command owns cleanup.

See [gateway chat](guides/gateway-chat.md) for configuration, ownership, commands,
limits, and durability. The server stores SDK JSONL snapshots at consequential
boundaries and mandatory audit records independently. Unfinished streaming text
can be lost on crash. Reads are bounded current views, not a durable cursor stream.
No second event database is required for this integration.

ADRs 0009 and 0011's exact replay and broader collaboration remain proposed work.
Remote TLS/device provisioning, uploads, and more provider adapters remain separate
features. Existing design proposals do not replace the implemented Agent contract.

**Identity/access contracts** (`crates/nessa-auth`) — reusable library, no binary.
Owns domain identities/memberships/credential metadata, boundary DTO validation,
and injected session authentication contracts. Embedded Cedar evaluates product policies through the application port. The local credential backend and guarded `/session` gateway are implemented.
See [local authentication](adr/done/0010-local-authentication.md) for setup and current limits. See the [crate guide](../crates/nessa-auth/README.md).

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
