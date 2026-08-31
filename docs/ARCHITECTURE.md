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
runtime will eventually drive. There is no agent in this repository yet; the
stand-in in `conversation.ts` exists so the bubbles, the typing dots, the
streaming reveal, and the composer's lit rim can be built and resized against
real UI.

## Code map

**Rust host** (`src-tauri/src/`) — everything that is the operating system's
opinion rather than the product's.

| File | Owns |
| --- | --- |
| `main.rs` | The composition root. Builds the app, wires the tray, shortcut, and window, and is the only place concrete pieces are chosen and handed to each other. |
| `host.rs` | The host/shell seam: event names and the `PanelSize` payload. The frontend lists the same names in `host-window.ts`; a test fails if they drift. |
| `panel.rs` | The panel frame: opening size, lower-right placement, show/hide. The tray and the shortcut request a toggle; they do not fit the frame. |
| `tray.rs` | The menu bar extra (macOS) or StatusNotifierItem (Linux), and the surface-toggle request. Creating it is survivable: a desktop with no tray still launches. |
| `shortcut.rs` | The global accelerator that summons and dismisses the panel. |
| `settings.rs` | The on-disk settings shape and its defaults. The file *is* the settings interface until there is a UI; `serde(default)` is what lets an older or hand-edited file still load. |
| `vibrancy.rs` | The native frosted surface on macOS, exposed to the frontend as one command. On Linux it is a no-op; the shell uses CSS `backdrop-filter` instead. |
| `viewport.rs` | Holds the page's viewport still during a live resize, on both WKWebView and WebKitGTK, by pinning a work-area-sized webview to the window's bottom right. |
| `live_resize.rs` | Forwards AppKit's live-resize notifications so the border glow can stay lit. On Linux, size-allocate while a mouse button is down is the drag. |

**React shell** (`src/`) — everything that is on screen.

| File | Owns |
| --- | --- |
| `app.tsx` | The panel chrome: the stage, the glow, the resize handle, the tab strip, the composer. It renders; it does not own conversation rules. |
| `transcript.tsx` | The turn list for the open conversation. Scrolls itself. |
| `conversation.ts` | The strip's rules: naming, drafts, the never-empty tab bar, `idle → thinking → streaming`. The stand-in reply lives here too. |
| `use-conversation.ts` | The strip's state and the stand-in clocks. One timer per conversation. |
| `use-host-panel.ts` | The host seam wiring: frost, tray surface request, composer focus. |
| `host-window.ts` | The single seam to the desktop host. Guarded so the same UI runs in a plain browser with the seam no-oping. |
| `use-surface.ts` | Which surface is chosen, and remembering it. The frontend owns this; the tray only *requests* a toggle and reflects the answer back. |
| `use-color-scheme.ts` | Light/dark following the system. |
| `use-edge-reveal.ts` | The border glow that follows the pointer, pinned for the length of a resize. |
| `use-panel-frame.ts` | Writes the host window's size into CSS, so the panel can be the window even when the viewport is not. |
| `agent-identity.ts` | The agent's name and avatar seed. |
| `waveform-icon.tsx` | The voice glyph in the composer. |

## Boundaries

Two, and they are both real:

**Host ↔ shell.** They talk over exactly one seam (`host-window.ts` on one side,
`host.rs` plus the command handlers on the other). Everything crossing it is an
explicit command or event, never shared state. This is what lets `pnpm dev`
open the UI in a browser with no host at all.

**Owner of a preference.** State has one owner and one direction. The surface
choice is owned by the frontend and reflected into the tray's check mark; the
tray requests, it does not decide. New preferences point the same way, or they
get an ADR explaining why not.

## Invariants

Things that must stay true. They are invisible in the code, which is why they
are written here.

- `main.rs` is the only composition point. Nothing else constructs a concrete
  dependency for someone else to use.
- The panel frame is reapplied on every show. Nothing may cache a frame across
  shows — that is the bug this design exists to prevent.
- The page's viewport does not move during a resize. The webview is pinned; the
  window moves over it; the shell is told the window's size.
- The window seam is guarded. Any new host call goes through `host-window.ts`
  and no-ops outside Tauri, or the browser workflow breaks silently.
- A settings file missing keys still launches. Any new key has a default.
- Failures at the edges — blur, sizing, tray, viewport — are reported and
  survivable, not fatal. The panel opening unblurred, or without a tray, beats
  the panel not opening.
- Linux is a first-class host. The panel is reachable without a menu bar: the
  taskbar is on, the window opens on launch, and close quits if there is no tray.
- A component either renders or coordinates, never both.
- There is no `utils` module, on either side.

## Cross-cutting

- **Failure policy:** edge failures degrade the surface, they do not stop the
  launch. Log with the `[nessa]` prefix and continue.
- **Platform code** is `cfg`-gated at the narrowest point that works, never
  spread through a module.
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
| A new host event or payload | `host.rs` and `host-window.ts` together |
| The conversation surface (tabs, turns, stand-in reply) | `conversation.ts` / `use-conversation.ts` |
| The transcript chrome | `src/transcript.tsx` |
| The panel chrome or composer | `src/app.tsx` |
| Resize jitter, the pinned webview | `viewport.rs` |
| The border glow | `use-edge-reveal.ts`, and `live_resize.rs` on macOS |
| Anything that talks to the host | `src/host-window.ts` — and only there |
| The agent runtime, when it lands | A new context; see *Growing a new context* in [codebase-structure.md](codebase-structure.md) |

## What is deliberately not here yet

There is no agent runtime, no persistence for conversations, no settings UI.
When those arrive they are new contexts with their own domain, not additions to
`conversation.ts` and `settings.rs`. The shape to grow into is in
[codebase-structure.md](codebase-structure.md); the trigger for applying it is a
concept acquiring an invariant, a second consumer, or its own storage.
