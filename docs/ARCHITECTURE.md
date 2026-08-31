# Architecture

The map of this repository: what the pieces are, where a change goes, and what
must stay true. Read this before your first change. Update it in the same change
that invalidates it.

For *how* to think about extending it, see the `system-architect` skill
(`.claude/skills/system-architect/`). For the rules applied to this repo, see
[codebase-structure.md](codebase-structure.md).

## The problem

Nessa is a menu bar agent. A transparent, floating panel that is summoned from
the tray or a global shortcut, hands the caret to a composer, and shows a
conversation with an agent. There is no Dock icon and no titlebar; the window
is a surface that appears over whatever the person is already doing and gets out
of the way again.

The two hard parts are **the window** — placing, sizing, and re-fitting a
chromeless panel across displays, with a native blur that can be turned off —
and **the conversation** — a turn list with an `idle → thinking → streaming`
lifecycle that a real agent runtime will eventually drive.

## Code map

**Rust host** (`src-tauri/src/`) — everything that is the operating system's
opinion rather than the product's.

| File | Owns |
| --- | --- |
| `main.rs` | The composition root. Builds the app, wires the tray, shortcut, and window, and is the only place concrete pieces are chosen and handed to each other. |
| `tray.rs` | The menu bar item and panel geometry: `anchor_to_edge` re-fits the frame on every show, so moving between displays re-places the panel rather than stranding it. |
| `shortcut.rs` | The global accelerator that summons and dismisses the panel. |
| `settings.rs` | The on-disk settings shape and its defaults. The file *is* the settings interface until there is a UI; `serde(default)` is what lets an older or hand-edited file still load. |
| `vibrancy.rs` | The native frosted surface, exposed to the frontend as one command. |

**React shell** (`src/`) — everything that is on screen.

| File | Owns |
| --- | --- |
| `app.tsx` | The conversation surface: the turn list, the phase, and the composer wiring. `draftReply` is the stand-in for the agent runtime and drives the same state a real one will. |
| `host-window.ts` | The single seam to the desktop host. Guarded so the same UI runs in a plain browser with the seam no-oping. |
| `use-surface.ts` | Which surface is chosen, and remembering it. The frontend owns this; the tray only *requests* a toggle and reflects the answer back. |
| `use-color-scheme.ts` | Light/dark following the system. |
| `agent-identity.ts` | The agent's name and avatar seed. |

## Boundaries

Two, and they are both real:

**Host ↔ shell.** They talk over exactly one seam (`host-window.ts` on one side,
the Tauri command handlers on the other). Everything crossing it is an explicit
command or event, never shared state. This is what lets `pnpm dev` open the UI
in a browser with no host at all.

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
- The window seam is guarded. Any new host call goes through `host-window.ts`
  and no-ops outside Tauri, or the browser workflow breaks silently.
- A settings file missing keys still launches. Any new key has a default.
- Failures at the edges — blur, sizing, tray — are reported and survivable, not
  fatal. The panel opening unblurred beats the panel not opening.
- There is no `utils` module, on either side.

## Cross-cutting

- **Failure policy:** edge failures degrade the surface, they do not stop the
  launch. Log with the `[nessa]` prefix and continue.
- **Platform code** is `cfg`-gated at the narrowest point that works, never
  spread through a module.
- **Nothing in the shell reaches the OS directly.** It goes through the seam.

## Where to make a change

| You want to change | Go to |
| --- | --- |
| Where or how big the panel appears | `tray.rs` (`anchor_to_edge`, `apply_configured_size`) |
| What the tray menu offers | `tray.rs`, plus the frontend if it owns the state |
| The summon shortcut | `settings.rs` for the key, `shortcut.rs` for registration |
| A new persisted preference | `settings.rs` (with a default), then its owner |
| The conversation surface or composer | `src/app.tsx` |
| Anything that talks to the host | `src/host-window.ts` — and only there |
| The agent runtime, when it lands | A new context; see *Growing a new context* in [codebase-structure.md](codebase-structure.md) |

## What is deliberately not here yet

There is no agent runtime, no persistence for conversations, no settings UI.
When those arrive they are new contexts with their own domain, not additions to
`app.tsx` and `settings.rs`. The shape to grow into is in
[codebase-structure.md](codebase-structure.md); the trigger for applying it is a
concept acquiring an invariant, a second consumer, or its own storage.
