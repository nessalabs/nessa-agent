# Codebase structure

The general rules live in the **`system-architect`** skill
([structure reference](../.claude/skills/system-architect/references/structure.md)) —
layout, dependency direction, the generic absences, boundaries between runtimes,
and how to grow a module. This file is only the part that is specific to Nessa,
and it is the part that changes as Nessa grows.

## Today

Nessa is a two-runtime desktop app: a Rust host (window, tray, shortcut,
settings, OS integration) and a React shell (chat surface, composer, avatar).
Right now both sides are small and flat, and that is correct — a five-file
module does not need a four-layer split. The shape below is what to grow *into*,
applied at the moment a module earns it, not before.

**The trigger:** a concept acquires an invariant, a second consumer, or its own
persistence. Until then, one well-named file.

## Target shape

```
src/                      React shell — presentation adapters only
  <feature>/              one folder per product surface (conversation, composer, …)
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

The generic list is in the skill. These are the Nessa-specific ones, and they
must stay true:

1. `domain/` contains no `tauri`, `serde`-transport, async-runtime, or
   filesystem import.
2. `main.rs` is the only composition point. Nothing else constructs a concrete
   dependency for someone else to use.
3. The panel frame is reapplied on every show. Nothing caches a frame across
   shows — that is the bug the design exists to prevent.
4. Every host call goes through `host-window.ts` and no-ops outside Tauri.
5. A settings file missing keys still launches; every new key has a default.
6. Edge failures — blur, sizing, tray, viewport — are reported and survivable,
   never fatal. The panel opening unblurred, or without a tray, beats the panel
   not opening.
7. The page's viewport does not move during a resize, on macOS or on Linux.
8. Linux is reachable without a menu bar: skip-taskbar is off, and the panel
   opens on launch.

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
  in `host.rs` and `src/host-window.ts`; a test in `host.rs` fails if a name on
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
  hook; rendering takes props and has no idea where they came from. `app.tsx`
  is the panel chrome; `use-conversation.ts` is the strip; `transcript.tsx` is
  the log.
- Design-system components are consumed, not wrapped "just in case". A wrapper
  with no behaviour is a layer that only forwards.
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
