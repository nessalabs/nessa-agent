# Codebase structure

The rules in [system-architect.md](system-architect.md) applied to this
repository specifically. This is the part that changes as Nessa grows; the
persona is the part that does not.

## Today

Nessa is a two-runtime desktop app: a Rust host (window, tray, shortcut,
settings, OS integration) and a React shell (chat surface, composer, avatar).
Right now both sides are small and flat, and that is correct — a five-file
module does not need a four-layer split. The rules below describe the shape to
grow *into*, applied at the moment a module earns them, not before.

**The trigger for applying them:** a concept acquires an invariant (something
that must always be true), or a second consumer, or its own persistence. Until
then, keep it in one file with a good name.

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

## Dependency direction

```
src/ (React)  ──►  contracts/  ◄──  adapters/  ──►  application/  ──►  domain/
```

- `domain/` imports nothing but itself and standard types.
- `application/` imports `domain/` and declares traits for what it needs.
- `adapters/` implement those traits and know about Tauri, the filesystem, the
  OS, the network.
- `contracts/` is the only path any other module or the frontend may import.

If an import in a diff crosses these arrows the wrong way, that is the review
comment — before correctness, before style.

## Naming

- Folder name == module name == the domain concept, in the product's words.
- No `utils`, `helpers`, `common`, `shared`, `core`, `types`, `misc`. If you
  cannot name the module after what it *is*, you have not found the concept.
- One canonical import path per item. No re-exports that create a second route.
- Types carry meaning: a `SessionId`, not a `String`; a `PanelWidth`, not an
  `f64`. The window geometry code in particular is a place where a wrong number
  in the right slot compiles happily.

## The absences

These must be true. They are invisible, so they are also the ones that erode.

1. `domain/` contains no `tauri`, `serde`-transport, async-runtime, or
   filesystem import.
2. No module imports another module's non-`contracts/` path.
3. The module graph is acyclic.
4. No global mutable state, singleton, or service locator. Dependencies are
   parameters, passed first.
5. No `#[cfg(test)]`-only constructor that builds a state production cannot.
6. No unbounded queue, channel, or buffer. Every one has a bound and a
   documented behaviour when full.
7. Nothing outside `adapters/` knows the on-disk settings format or the IPC
   payload shape.

When the codebase is large enough for these to be worth automating, they become
a test (see [testing-strategy.md](testing-strategy.md), *structure tests*).

## The core and what sits on it

Nessa will grow a core — the agent runtime, the turn lifecycle, the transport to
whatever produces replies — and a set of product surfaces on top of it: the
panel, the composer, the tray, whatever comes after. The rule from §8 of the
persona applies literally here:

- The runtime core has no knowledge that a menu bar panel exists. No `if
  panel`, no field only the tray sets, no enum variant named after a surface.
- A surface composes core pieces and adds its own rules. It depends on the core;
  the core never depends on it.
- When a surface needs something the core cannot express, the core gains a
  *general* capability — a port, an event, a parameter naming a concept the core
  already has — and the surface supplies the specific part.
- The test: could this core piece serve a Nessa with no menu bar at all — a CLI,
  a second window, a background run? If not, it has been contaminated.

The same applies inside the host today. `tray.rs` owns geometry and the menu;
it does not own conversation state, and it does not decide preferences — it
requests and reflects. Keep new code pointing the same way.

## Rules for the host/shell seam

The one boundary that already exists, and the one most likely to rot silently.

- **One definition of every payload shape, imported by both sides.** Never
  redeclare the shape of a command argument or event on the receiving side. Two
  declarations of the same contract drift with no error anywhere — the compiler
  is happy on both sides right up until runtime. Generate the frontend types from
  the Rust definitions, or keep one hand-written declaration that both sides
  import; not two.
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

## Failure-first checklist for host code

Before any new code that touches the filesystem, the OS, or another process:

- What must stay true if it stops halfway? Which function guarantees it?
- What if it runs twice — two shows, a double shortcut press, a restart mid-write?
- What if two of them run at once? Impossible by construction, serialised by one
  owner, or a documented benign race — pick one.
- Is the durable write ordered before the announcement?
- What is the bound on every queue, channel, and retry?
- Is this failure fatal or survivable? Match the existing policy: edge failures
  degrade the surface and are logged; they do not stop the launch.

Settings writes are the current instance of most of this: a partial write must
not produce a file that fails to load, which is what `serde(default)` plus
writing the full defaults on first launch is buying.

## Cross-context communication

Prefer an event over a call. When one context needs another to act, it publishes
a fact ("reply completed") rather than issuing an instruction ("update the
transcript"). The publisher then has no knowledge of who reacts, and the set of
reactors can change without touching it.

Direct calls are acceptable when the relationship is genuinely a dependency
rather than a collaboration — but the caller defines the trait and the callee
implements it, so the arrow points where the design wants it, not where the file
happens to live.

## Frontend specifics

- A component either renders or coordinates, never both. Coordination lives in a
  hook; rendering takes props and has no idea where they came from.
- Design-system components are consumed, not wrapped "just in case". A wrapper
  with no behaviour is a layer that only forwards.
- Host-window interaction goes through one seam (as it already does), so the UI
  runs in a plain browser with the seam no-oping. Keep that property: it is what
  makes design work fast, and it is a real architectural boundary, not a
  convenience.
- Persisted UI preference is state with an owner. The frontend owning the
  surface choice and the tray reflecting it — rather than the tray owning it —
  is the right direction; keep new preferences pointing the same way.

## Growing a new context

1. Write the name and the one-sentence purpose in
   [ARCHITECTURE.md](ARCHITECTURE.md) first. If it is hard to write, stop.
2. Create `domain/` with the invariant enforced in a constructor, and a test.
3. Add the use case in `application/`, with a trait for anything it needs from
   the outside.
4. Implement the trait in `adapters/`.
5. Expose the minimum in `contracts/`.
6. Wire it in exactly one composition point.

Step 6 matters: there is one place where concrete adapters are chosen and
handed to use cases. Everything else receives what it needs. That single place
is what makes the system testable and what makes swapping an edge a one-file
change.
