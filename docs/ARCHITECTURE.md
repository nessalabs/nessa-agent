# Architecture

The map of this repository: what the pieces are, where a change goes, and what
must stay true. Read this before your first change. Update it in the same change
that invalidates it.

For *how* to think about extending it, see the `system-architect` skill
(`.claude/skills/system-architect/`). For the rules applied to this repo, see
[codebase-structure.md](codebase-structure.md).

## The problem

Nessa is a floating chat panel. On macOS it is a menu bar extra: a transparent
surface summoned from the tray or a global shortcut, with no Dock icon and no
titlebar. On Linux it is the same panel, reachable from the system tray when
the desktop provides one and from the taskbar otherwise — there is no menu bar
to hang from, and a hidden window with no taskbar entry would be undiscoverable.

The two hard parts are **the window** — placing, sizing, and re-fitting a
chromeless panel across displays without the page's contents jittering during a
resize, with a frost that can be turned off — and **the conversation surface**
— a turn list with an `idle → thinking → streaming` lifecycle that a real agent
runtime will eventually drive. There is no chat RPC in this repository yet; the
conversation vertical keeps tabs and drafts as a UI session, and send is a
no-op until a remote gateway owns turns.

## Code map

**Rust host** (`src-tauri/src/`) — everything that is the operating system's
opinion rather than the product's.

| File | Owns |
| --- | --- |
| `main.rs` | The composition root. Builds the app, wires the tray, shortcut, and window. It never mentions macOS or Linux: OS behaviour is injected through `platform::current()`. |
| `host.rs` | The host/shell seam: event names and the `PanelSize` payload. The frontend lists the same names in `src/host/window.ts`; a test fails if they drift. |
| `panel.rs` | The panel frame: opening size, lower-right placement, show/hide. The tray and the shortcut request a toggle; they do not fit the frame. |
| `tray.rs` | The menu bar extra (macOS) or StatusNotifierItem (Linux), and the surface-toggle request. Creating it is survivable: a desktop with no tray still launches. |
| `shortcut.rs` | The global accelerator that summons and dismisses the panel. |
| `settings.rs` | The on-disk settings shape and its defaults. The file *is* the settings interface until there is a UI; `serde(default)` is what lets an older or hand-edited file still load. |
| `platform/` | The OS host. `Host` is the contract; `current()` injects one implementation for the compiled target. Commands `set_frosted` and `panel_size` live here too. |
| `platform/macos/` | Accessory app, `NSVisualEffectView` frost, WKWebView pin, AppKit live-resize notifications, hide-on-blur in release. |
| `platform/linux/` | WebKit DMA-BUF prep, GtkFixed pin, CSS frost (no-op natively), allocate-based live resize, shown on the taskbar at launch. |
| `platform/other/` | Webview fills the window; size events only. |

**Launch** ([justfile](../justfile)) — `just server` / `just dev` / `just web` / `just fast` / `just release`. Bundle names and Linux WebKit/GTK checks live in the justfile, not a second host layer. Windows recipes are written, not yet run on a Windows box.

**React shell** (`src/`) — everything that is on screen.

| Path | Owns |
| --- | --- |
| `main.tsx`, `store.ts` | Composition root. Mounts the panel, the session lifecycle, and product projections. |
| `conversation/` | The conversation vertical. See the table below. |
| `session/` | Wire session to `nessa-server` via `@nessa/client` (S1: connect + health). |
| `panel/` | The floating-window chrome. See the table below. |
| `host/` | Injected host features and the window seam (`window.ts`). |

**Conversation vertical** (`src/conversation/`) — one feature, independently testable.

| Path | Owns |
| --- | --- |
| `model/` | Shared language: `Conversation`, `Turn`, `ConversationTabs` (`conversations` + `activeId`). Discriminated turns and phases. No id mill. |
| `application/local-tabs.ts` | UI-session store shape: the shared tabs plus local id counters. A remote gateway mints its own ids and this type goes away. |
| `application/usecases/` | One file per command. Session: draft, active tab, open, close. Send/stop are no-ops until chat RPCs exist. |
| `application/ports.ts` | `ConversationGateway` — what the panel may ask the product to do. |
| `adapters/gateway/local.ts` | In-process UI-session gateway. Tomorrow this is the remote gateway. |
| `adapters/store/` | Redux projection. Reducers call the gateway; they do not contain rules. |
| `ui/` | Transcript, thinking pill, `useConversation`. Paints and dispatches. |
| `model/identity.ts` | The agent's name, seed, and hue wheel. |

**Session vertical** (`src/session/`) — WebSocket control-plane connection.

| Path | Owns |
| --- | --- |
| `model/` | `SessionPhase`, status copy for the empty state. |
| `adapters/client/` | `connectDevSession` (closes on health failure) + `getSessionClient` handle (live client outside Redux). |
| `adapters/store/` | Redux projection of connection status (`hello` / `health` only). |
| `adapters/lifecycle/` | Mount/reconnect effect, owned by the composition root. Subscribes `onClose` before publishing ready. |
| `ui/use-session.ts` | Hook the panel reads for status. |

Chat adapters must use `getSessionClient()` from the session barrel — do not open a second socket, and do not put `NessaClient` in Redux.

**Panel vertical** (`src/panel/`) — the floating window, not the product.

| Path | Owns |
| --- | --- |
| `model/` | `Surface` — frosted or clear. |
| `adapters/` | Host subscriptions: colour scheme, edge reveal, panel frame, frost, remembered surface, compositor flush. |
| `ui/app.tsx` | The chrome: stage, glow, resize handle, tab strip, composer. Renders; no effects. |
| `ui/waveform-icon.tsx` | The voice glyph in the composer. |

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
- Product state an agent must drive lives in the Redux store. Host, DOM, and clocks stay in adapters. See [adr/0001-redux-toolkit-for-product-state.md](adr/0001-redux-toolkit-for-product-state.md) and [adr/0003-panel-vertical.md](adr/0003-panel-vertical.md).

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
| The agent runtime, when it lands | A new context; see *Growing a new context* in [codebase-structure.md](codebase-structure.md) |

## What is deliberately not here yet

There is no agent runtime, no chat RPCs, no persistence for conversations, no
settings UI. The panel already opens a `stage=dev` `@nessa/client` session for
connect/health. When chat arrives it is a remote `ConversationGateway`, not an
addition to the local session adapter. See
[adr/0002-conversation-vertical-and-gateway.md](adr/0002-conversation-vertical-and-gateway.md).
