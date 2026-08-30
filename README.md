# Nessa

A menu bar agent: a transparent, floating chat panel built with Tauri 2, React 19,
TypeScript, and the [Nessa UI](https://github.com/nessalabs/nessa_ui) design system.

The window has no titlebar and no Dock icon. It lives behind the menu bar item —
click it to summon the panel, click again (or click away, in a release build) to
dismiss it, or press **⌘⇧A** from anywhere. Summoning it hands the caret
straight to the composer. The panel's header is just the agent's face and name,
and the whole strip drags the window; everything else lives in the tray menu.

It opens in the **lower right** of whichever screen it is summoned on, the way
Clawdia's panel does — a 420pt column filling the work area's height by default,
both configurable (see [Settings](#settings)). A panel shorter than the screen
sits on the bottom edge rather than hanging from the top. The frame is reapplied
on every show, so moving between displays re-fits it rather than stranding it
(`anchor_to_edge` in [src-tauri/src/tray.rs](src-tauri/src/tray.rs)).

## What is here

- **The iMessage-style chat surface** — `ChatMessage` / `ChatBubble` /
  `ChatMessageReceipt` / `ChatTypingIndicator` from Nessa UI: sent bubbles right,
  received left, typing dots while the agent thinks, a streamed reveal as the
  reply arrives.
- **The pill composer** — `PillComposer` / `PillComposerRow`, whose rim lights
  with traveling iridescence while the agent works. Enter is the send affordance,
  as the component intends, so the trailing slot carries a voice control instead —
  inert until there is a runtime to transcribe into — which hands over to a stop
  control while a reply is arriving.
- **The agent's face** — `RandomAvatar`, a deterministic generative avatar
  painted from the seed `"nessa"`. The app icon is the same painting, rasterized
  (see [Regenerating the icon](#regenerating-the-icon)).
- **Two surfaces** — the tray menu's **Transparent** item switches between the
  frosted surface, which blurs whatever the panel was summoned over, and the
  clear one, which removes the panel entirely so only the bubbles and the pill
  hang over the desktop. The frontend owns the choice and remembers it
  ([src/use-surface.ts](src/use-surface.ts)); the tray item only *requests* a
  toggle, and its check mark is reflected back from `set_frosted`.
- **No agent** — `draftReply` in [src/app.tsx](src/app.tsx) is a stand-in that
  drives the same state a real runtime will drive: a turn list plus an
  `idle → thinking → streaming` phase.

## Running it

```bash
pnpm install
pnpm app
```

`pnpm app` is `tauri dev`; it starts Vite on port 1420 and builds the Rust side.
`pnpm dev` alone opens the same UI in a plain browser, which is useful for design
work — the window controls no-op there (see [src/host-window.ts](src/host-window.ts)).

| Script | What it does |
| --- | --- |
| `pnpm app` | Run the desktop app (dev) |
| `pnpm app:fast` | A release build with the slow optimisations off — for testing |
| `pnpm app:build` | The shipping bundle (`.app` + `.dmg`) |
| `pnpm dev` | The UI in a browser, no Tauri |
| `pnpm typecheck` | `tsc --noEmit` |
| `pnpm ui:types` | Rebuild the linked package's `.d.ts` (see below) |

### Settings

There is no settings UI yet, so `settings.json` in the app's config directory
(`~/Library/Application Support/so.nessa.app/` on macOS) *is* the interface. It is
written with its defaults on first launch so the keys are discoverable, and
`serde(default)` fills in anything a later build adds or a person deletes. A
malformed file is reported and ignored rather than overwritten — throwing away
what someone was mid-way through typing is worse than falling back.

```json
{
  "toggleShortcut": "CmdOrCtrl+Shift+A",
  "panel": {
    "width": 420,
    "height": null,
    "minWidth": 420
  }
}
```

| Key | Meaning |
| --- | --- |
| `toggleShortcut` | Tauri accelerator syntax; `CmdOrCtrl` resolves per platform |
| `panel.width` | The width the panel *opens* at. After that the window's own width wins, so a drag on the resize edge is not thrown away |
| `panel.height` | The height it opens at. `null` fills whatever the work area leaves once the menu bar and the Dock have taken theirs, and keeps re-filling it across displays |
| `panel.minWidth` | How narrow the resize edge may drag it |

A configured `width` below `minWidth` is a contradiction, so the minimum wins —
it is what the resize edge enforces anyway. Height has its own floor
(`MIN_PANEL_HEIGHT`, 320): a panel shorter than that has no transcript left.

The file is rewritten with the merged result on every load, so keys a later build
adds appear in it without resetting the values already there. A shortcut that
will not parse, or that another app already owns, is reported and skipped rather
than fatal: the tray icon still opens the panel
([src-tauri/src/shortcut.rs](src-tauri/src/shortcut.rs)).

### Resizing an undecorated window

`decorations: false` means macOS gives the window no resize border, so
`resizable: true` on its own leaves nothing to grab — which is why dragging the
edge did nothing. The panel therefore carries its own handle: a strip down the
left edge that calls `startResizeDragging("West")`. West only, because the panel
is pinned to the right of the screen and spans the work area's height, so width
is the one dimension it owns.

`anchor_to_edge` re-fits the panel on every show, and it deliberately keeps the
window's *current* size rather than resetting it to the configured one —
otherwise a resize would be discarded the next time the panel was dismissed. The
configured geometry is applied once, at startup, by `apply_configured_size`.

## Working on a feature

Every agent starts here rather than running `git worktree` by hand:

```bash
./scripts/worktree.sh create add-something   # branch + worktree, ready to build
./scripts/worktree.sh clean                  # rebuild this crate, keep deps
./scripts/worktree.sh list
./scripts/worktree.sh remove add-something   # the branch is kept
```

A fresh worktree does not pay for a rebuild. It shares the main checkout's
`src-tauri/target`, so cargo reuses the ~500 already-compiled dependency crates
and only recompiles this app's own crate — measured at **27 s** in a new
worktree, against minutes from scratch. pnpm hardlinks from its global store, so
`pnpm install` costs seconds and no disk.

Cargo locks the shared target, so two worktrees building at once queue rather
than corrupt each other.

A bare `cargo clean` in any worktree does empty it for all of them, and that is
not preventable — cargo has no notion of a protected shared target. It is
**bounded** rather than fixed: sccache's cache lives in
`~/Library/Caches/Mozilla.sccache`, outside the target directory entirely, so
the worst case is one ~45 s rebuild rather than a cold one. Use
`./scripts/worktree.sh clean` instead: it runs `cargo clean -p nessa-app`, which
drops only this app's crate — the thing that is actually stale after a code
change — and rebuilds in **4 s** with every dependency intact.

Worktrees are created as **siblings** of this checkout
(`../nessa-app-<name>`), and that is load-bearing rather than cosmetic: the
design system is a relative `link:../nessa/…` dependency, so a worktree nested
any deeper resolves that path to nothing and fails to install.

## Build

Nothing ships that the app does not reach, and the two build modes exist so
testing does not cost a shipping build.

| | `.app` | Compile | What it is |
| --- | --- | --- | --- |
| `pnpm app:fast` | ~9 MB | ~45 s warm | `opt-level=1`, no LTO, no strip, no dmg |
| `pnpm app:build` | 6.5 MB | ~2 min | `opt-level=3`, fat LTO, one codegen unit, stripped |

`app:fast` overrides the release profile with `CARGO_PROFILE_RELEASE_*` env vars
rather than defining a second profile, so there is one definition and no chance
of the two drifting. (The Tauri CLI has no `--profile` flag, so a real second
cargo profile could not be selected anyway.) Both are release builds — neither
carries debug assertions — so what you test behaves like what you ship.

**sccache** caches compilation across profiles and checkouts
([src-tauri/.cargo/config.toml](src-tauri/.cargo/config.toml)). Rebuilding from
clean went 104s → **43s** at an 85% hit rate. It needs `sccache` on `PATH`
(`brew install sccache`); without it cargo fails to spawn the wrapper, so delete
that file if you would rather not have it. It cannot cache incrementally-compiled
crates, so it skips this app's own crate in dev builds — the win is the ~500
dependency crates, which is where the time goes.

Dev builds use `debug = "line-tables-only"`: full debug info is the single
biggest cost in a Tauri rebuild, and line tables still give a readable backtrace.

### The edit cycle

Measured on this machine, with `pnpm app` running:

| Change | Cost | What happens |
| --- | --- | --- |
| Anything under `src/` | **76 ms** | Vite HMR. No Rust rebuild, no restart, React state kept |
| Anything under `src-tauri/src/` | **2.8 s** | Incremental rebuild (~1.7 s) and the app relaunches |
| A dependency version | seconds | Cargo rebuilds that crate and its dependents, not the graph |

So keep iteration in the frontend where the work allows it: that path is
sub-100ms and does not lose the conversation on screen.

There is no hot-*patching* of the Rust side, and it is not worth adding. Swapping
code into a running process means building the app as a reloadable dylib
(`hot-lib-reloader`, Dioxus's `subsecond`), which fights Tauri's runtime setup
and would cost more to maintain than the 2.8 s it saves. The relaunch is already
faster than the webview takes to paint.

### Only what we need

The frontend is **576 KB**, all of it reached — down from 7.5 MB.

The package's published bundle is a single `dist/index.js`, which hoists every
dependency to one module's top level, so `mermaid` and `katex` are *static*
imports of that one file. Rollup cannot drop them by tree-shaking the components
that use them: an app using six components shipped mermaid, cytoscape, and the
whole KaTeX font set. Two changes fix it:

1. **Bundle from the package's source, not its `dist`** (aliases in
   [vite.config.ts](vite.config.ts)) — every component is its own module again,
   so unused ones shake out.
2. **Import the component modules directly**, not the barrel. Vite emits a
   module's CSS and font assets as soon as it *transforms* it, before Rollup can
   shake it — so reaching `math-block` through the barrel shipped 1.2 MB of
   KaTeX fonts even though nothing rendered math.

**When mermaid or math are actually needed**, import them with `React.lazy` from
their own module rather than adding them to a static import. They then become
chunks fetched the first time a message contains a diagram, instead of weight
every launch pays for.

Types still come from the package's built `.d.ts`, not its source: typechecking
its source pulls in the copy of React's types under its own `node_modules`, and
two copies make identical types nominally incompatible. `pnpm ui:types` refreshes
them after changing the package. `noUnusedLocals` is off for the same reason —
consuming the design system as source puts its files in this program, and its
dead locals are not this app's to fix.

### The frosted surface is native, not CSS

The frost is an `NSVisualEffectView` behind the webview
([src-tauri/src/vibrancy.rs](src-tauri/src/vibrancy.rs)), not a CSS
`backdrop-filter`. On a transparent, undecorated macOS window the CSS filter does
not sample the behind-window content 1:1 — it stretches it into a bleed running a
few hundred points down the panel — and it stops updating when the window loses
focus, so the surface visibly changed as you clicked away. The native view does
both correctly, and `NSVisualEffectState::Active` keeps it frosted whether or not
the panel is frontmost.

Two consequences for the CSS: the panel carries only a *tint* over the native
frost, and it fills the window edge to edge, because the effect is clipped to an
18pt rounded rect of the window — any padding would leave it poking out past the
panel. Since the effect is window-level, the clear surface has to turn it off
natively too, which is what the `set_frosted` command is for.

### Hide-on-blur

A menu bar panel normally dismisses when you click away, but that would hide the
window every time you open devtools. It is therefore release-only — see the
`Focused(false)` arm in [src-tauri/src/main.rs](src-tauri/src/main.rs).

## The Nessa UI dependency

`PillComposer` and the chat bubbles have **not landed on the design system's main
branch yet** — they live on `claude/imessage-composer-chat-ui-be1e6a`. So the app
depends on a git worktree of that branch rather than on npm:

```
"@nessa-ui/react": "link:../nessa/.claude/worktrees/imessage-composer-chat-ui/packages/react"
```

That worktree is checked out detached at the branch tip. To move it forward:

```bash
git -C ../nessa/.claude/worktrees/imessage-composer-chat-ui checkout claude/imessage-composer-chat-ui-be1e6a
pnpm ui:build
```

When the branch merges and the package publishes, this becomes a plain
`"@nessa-ui/react": "^x.y.z"` and the worktree can go away.

### Why the app compiles the design system's CSS from source

[src/styles.css](src/styles.css) imports `@nessa-ui/react/src/app.css`, not the
package's prebuilt stylesheet. The built CSS only carries the utilities the
library's own components happen to use, so a class the *app* writes — `size-14`,
say — silently resolves to nothing. Compiling from source gives the app the full
utility set and the same tokens. It reverts to the published stylesheet once the
package ships to npm.

React is deduped in [vite.config.ts](vite.config.ts): the linked checkout carries
its own React, and without that the app and the library would each load a copy
and every hook in the library would throw.

## Regenerating the icon

`pnpm dev` serves **/icon.html** ([src/icon-preview.tsx](src/icon-preview.tsx)), a
dev-only page that renders candidate palettes at icon size *and* at menu-bar size
over a menu-bar blue — a palette that reads well at 96px can collapse into a blob
at 16px, which is how the first icon went wrong: the default hue wheel put a
near-white wash against a dark ground, and the menu bar showed a white patch. The
shipped wheel is `AGENT_HUES` in
[src/agent-identity.ts](src/agent-identity.ts), shared with the in-app avatar so
the two cannot drift.

The icon itself is [src-tauri/icons/nessa-avatar.svg](src-tauri/icons/nessa-avatar.svg),
lifted from the preview's `sorbet · pastel · shipped` tile — a rendered
`RandomAvatar` with `AGENT_HUES`, `AGENT_ICON_TONE`, and `ground="paper"` — with
its Tailwind blend-mode classes inlined so the file stands alone. Being pastel it
sits quietly on a dark or coloured menu bar and goes faint on a light one; a
heavier `tone` in `AGENT_ICON_TONE` trades the sorbet for contrast. To rebuild
the set:

```bash
qlmanage -t -s 1024 -o /tmp src-tauri/icons/nessa-avatar.svg && cp /tmp/nessa-avatar.svg.png src-tauri/icons/nessa-avatar.png && pnpm tauri icon src-tauri/icons/nessa-avatar.png
```

## Next

- Wire a real agent to `draftReply` and drive `phase` from stream events.
- Make the voice control real: the design system's story streams a transcription
  into the input word by word, with hold-to-record and a live meter.
- Persist the transcript across launches.
- The rest of the chat kit — attachments, tapbacks, reply threads, chat tabs —
  is already in the design system; see its `pill-composer` Storybook story.
