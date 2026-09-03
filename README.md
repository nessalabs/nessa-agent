# Nessa

A menu bar agent: a transparent, floating chat panel built with Tauri 2, React 19,
TypeScript, and the [Nessa UI](https://github.com/nessalabs/nessa_ui) design system.

The window has no titlebar and no Dock icon. It lives behind the menu bar item —
click it to summon the panel, click again (or click away, in a release build) to
dismiss it, or press **⌘⇧A** from anywhere. Summoning it hands the caret
straight to the composer. The tab strip doubles as the titlebar — the gaps
around the tabs drag the window; everything else lives in the tray menu.

It opens in the **lower right** of whichever screen it is summoned on, the way
Nessa's panel does — a 420pt column filling the work area's height by default,
both configurable (see [Settings](#settings)). A panel shorter than the screen
sits on the bottom edge rather than hanging from the top. The frame is reapplied
on every show, so moving between displays re-fits it rather than stranding it
(`anchor_to_edge` in [src-tauri/src/panel.rs](src-tauri/src/panel.rs)).

On **Linux** the same panel is a floating window. There is no menu bar extra to
hang from, so it opens on launch, stays on the taskbar, and still summons from
the system tray and **Ctrl+Shift+A** when those exist. Frost is CSS
`backdrop-filter` rather than an `NSVisualEffectView`. The webview is pinned
the same way as on macOS so a resize does not jitter the composer.

OS-specific window behaviour is not scattered through `main`. It lives in
[`src-tauri/src/platform/`](src-tauri/src/platform/) — a `Host` trait with one
implementation per OS, injected by `current()` — and in
[`src/host/`](src/host/) on the shell.

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
- **Tabs** — `ChatTabs`, one per conversation. Each carries its own
  `RandomAvatar`, painted from the tab's id, and a busy dot while it is
  mid-reply. A conversation is named after its opening line. Transcripts,
  phases *and drafts* belong to the conversation, not to the panel: a reply
  keeps arriving in a background tab, and switching tabs mid-sentence does not
  carry the sentence into someone else's thread.
- **Two surfaces** — the tray menu's **Transparent** item switches between the
  frosted surface, which blurs whatever the panel was summoned over, and the
  clear one, which removes the panel entirely so only the bubbles and the pill
  hang over the desktop. The frontend owns the choice and remembers it
  ([src/panel/adapters/surface.ts](src/panel/adapters/surface.ts)); the tray item only *requests* a
  toggle, and its check mark is reflected back from `set_frosted`.
- **No agent** — the local conversation gateway in
  [`src/conversation/`](src/conversation/) stands in for the server. It drives
  the same strip a real runtime will drive: a turn list plus an
  `idle → thinking → streaming` phase. The panel only paints that projection.

## Running it

```bash
pnpm install
just dev
```

[`just`](https://just.systems) is the entry ([justfile](justfile)). `just`
lists recipes. `just dev` is `tauri dev` when a display is available, the
browser UI (`just web`) when it is not. `just release` is the shipping
installer. `pnpm app` and `pnpm dev` still work without those defaults. The
window controls no-op in the browser (see [src/host/window.ts](src/host/window.ts)).

Install `just` with the platform's package manager (`apt install just`,
`brew install just`, `winget install --id Casey.Just --exact`).

### Linux

The lockfile needs **Rust 1.85+** (edition 2024 crates). Ubuntu's packaged
`rustc` is often 1.83; install via rustup. [`rust-toolchain.toml`](rust-toolchain.toml)
pins `stable`, so `just dev` and `cargo test` pick it without an extra env var.

Build packages on Debian/Ubuntu:

```bash
sudo apt install libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev patchelf fakeroot
```

Then `just dev`. The Linux recipe refuses to start the desktop app if those
packages are missing, and it disables WebKit's DMA-BUF renderer on a VNC or
software X server that has no DRM device. To force that path on a machine
that does have `/dev/dri`:

```bash
WEBKIT_DISABLE_DMABUF_RENDERER=1 WEBKIT_DISABLE_COMPOSITING_MODE=1 just dev
```

A testing-shaped `.deb` is `just fast`. A shipping `.deb` is `just release`.

### macOS

`just dev` is `tauri dev`. `just fast` writes a `.app` (no dmg). `just release`
writes a `.dmg`.

### Windows

Windows recipes in the justfile have **not been run on a Windows machine yet**:
`just fast` and `just release` ask Tauri for `nsis`. The justfile uses
`cmd.exe` so Git's `sh` is not required. Please verify `just dev`, `just fast`,
and `just release` there.

| Command | What it does |
| --- | --- |
| `just` | List recipes |
| `just dev` | Desktop app in dev mode (falls back to the browser UI with no display) |
| `just web` | The UI in a browser, no Tauri |
| `just fast` | Testing-shaped release — slow opts off (`.app` / `.deb` / NSIS) |
| `just release` | Shipping bundle — fat LTO, stripped (`.dmg` / `.deb` / NSIS) |
| `pnpm app` | `tauri dev`, no host defaults |
| `pnpm app:build` | The shipping bundle for every Linux format (`.deb` + `.rpm` + AppImage) |
| `pnpm typecheck` | `tsc --noEmit` |
| `pnpm ui:types` | Pull the vendored `@nessa-ui/react` checkout forward |

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
workspace `target/`, so cargo reuses the ~500 already-compiled dependency crates
and only recompiles this repo's own crates — measured at **27 s** in a new
worktree, against minutes from scratch. pnpm hardlinks from its global store, so
`pnpm install` costs seconds and no disk.

Cargo locks the shared target, so two worktrees building at once queue rather
than corrupt each other.

A bare `cargo clean` in any worktree does empty it for all of them, and that is
not preventable — cargo has no notion of a protected shared target. It is
**bounded** rather than fixed: sccache's cache lives in
`~/Library/Caches/Mozilla.sccache`, outside the target directory entirely, so
the worst case is one ~45 s rebuild rather than a cold one. Use
`./scripts/worktree.sh clean` instead: it runs
`cargo clean -p nessa-app -p nessa-server`, which drops only this repo's crates
— the things that are actually stale after a code change — and rebuilds in
**4 s** with every dependency intact.

Worktrees are created as **siblings** of this checkout
(`../nessa-app-<name>`) so they can share this repo's workspace `target/`.

## Build

Nothing ships that the app does not reach, and the two build modes exist so
testing does not cost a shipping build.

| | Artifact | Compile | What it is |
| --- | --- | --- | --- |
| `just fast` | ~9 MB `.app` / a `.deb` | ~45 s warm | `opt-level=1`, no LTO, no strip, no dmg / AppImage |
| `just release` | 6.5 MB `.app` inside a `.dmg` / a `.deb` | ~2 min | `opt-level=3`, fat LTO, one codegen unit, stripped |

`just fast` / `pnpm app:fast` overrides the release profile with
`CARGO_PROFILE_RELEASE_*` env vars rather than defining a second profile, so
there is one definition and no chance of the two drifting. (The Tauri CLI has
no `--profile` flag, so a real second cargo profile could not be selected
anyway.) `just release` leaves that profile alone (`opt-level=3`, fat LTO,
strip) and asks for the shipping installer (`dmg` / `deb` / `nsis`). The
justfile names the bundle so the Linux CLI is not asked for macOS's `app`
or `dmg`. Both are release binaries — neither carries debug assertions — so
what you test behaves like what you ship.

**sccache** caches compilation across profiles and checkouts when
`RUSTC_WRAPPER=sccache` is set in the environment. It is optional: without it
cargo uses the ordinary compiler, which is what Linux and CI need. Rebuilding
from clean went 104s → **43s** at an 85% hit rate on the machine that measured
it. `brew install sccache` or `apt install sccache`; it cannot cache
incrementally-compiled crates, so it skips this app's own crate in dev builds —
the win is the ~500 dependency crates, which is where the time goes.

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
([src-tauri/src/platform/macos/vibrancy.rs](src-tauri/src/platform/macos/vibrancy.rs)), not a CSS
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
window every time you open devtools. It is therefore release-only — see
`on_window_event` on the macOS host in
[src-tauri/src/platform/macos/mod.rs](src-tauri/src/platform/macos/mod.rs).

## The Nessa UI dependency

The chat kit lives in [`nessalabs/nessa_ui`](https://github.com/nessalabs/nessa_ui)
(`packages/react`). It is not on npm yet, so `pnpm install` links it from
`.vendor/nessa_ui`. That directory is filled by `scripts/ensure-nessa-ui.mjs`
before install: a sibling `nessa_ui` (or the original imessage worktree) is
symlinked if present, otherwise the repo is cloned.

```
"@nessa-ui/react": "link:.vendor/nessa_ui/packages/react"
```

To move the clone forward:

```bash
pnpm ui:types
```

When the package publishes, this becomes `"@nessa-ui/react": "^x.y.z"`.

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
[src/conversation/model/identity.ts](src/conversation/model/identity.ts), shared with the in-app avatar so
the two cannot drift.

### The tray icon is a different painting

The menu bar gets its own icon, [src-tauri/icons/tray-avatar.svg](src-tauri/icons/tray-avatar.svg)
→ `tray-icon.png`, compiled into the binary with `include_bytes!` so dev and
packaged builds load identical bytes with no resource-path lookup.

Same seed and hue wheel as the app icon, but `tone="vivid"` instead of
`"pastel"`. Pastel washes sit around 0.87–0.97 lightness: delicate and correct in
the Dock at 128pt, but at the 16pt the menu bar actually draws they collapse into
a pale disc that reads as a white blob on any bar. Dropping the avatar's `paper`
ground does *not* fix that — the pigment itself is near-white at that tone, so
the weight has to change, not the backing.

Regenerate it from the preview's `sorbet · vivid` tile, at 44px (the 22pt menu
bar slot at 2×). Because it is compiled in, **cargo does not notice the new
bytes on its own** — touch the file that includes it:

```bash
node scripts/render-icon.mjs src-tauri/icons/tray-avatar.svg src-tauri/icons/tray-icon.png 44 && touch src-tauri/src/tray.rs
```

The icon itself is [src-tauri/icons/nessa-avatar.svg](src-tauri/icons/nessa-avatar.svg),
lifted from the preview's `sorbet · pastel · shipped` tile — a rendered
`RandomAvatar` with `AGENT_HUES`, `AGENT_ICON_TONE`, and `ground="paper"` — with
its Tailwind blend-mode classes inlined so the file stands alone. Being pastel it
sits quietly on a dark or coloured menu bar and goes faint on a light one; a
heavier `tone` in `AGENT_ICON_TONE` trades the sorbet for contrast. To rebuild
the set:

```bash
node scripts/render-icon.mjs src-tauri/icons/nessa-avatar.svg src-tauri/icons/nessa-avatar.png 1024 && pnpm tauri icon src-tauri/icons/nessa-avatar.png
```

### Do not rasterise icons with `qlmanage`

The obvious macOS one-liner, `qlmanage -t -s 1024 -o . icon.svg`, is a QuickLook
**thumbnailer**: it composites the artwork onto white. The PNG it writes has an
alpha channel — `sips -g hasAlpha` cheerfully reports `yes` — but every pixel is
opaque, so the icon carries a white square into the menu bar and the Dock. That
check is not evidence of transparency; read a corner pixel instead.

[scripts/render-icon.mjs](scripts/render-icon.mjs) uses resvg, which renders the
SVG itself and leaves everything outside the artwork transparent. It converts the
design system's `oklch()` colours to sRGB first: resvg does not implement CSS
Color 4, and without the conversion every fill resolves to black and the avatar
comes out a solid disc.

## Next

- Replace `adapters/gateway/local.ts` with a remote `ConversationGateway` and drive `phase` from stream events.
- Make the voice control real: the design system's story streams a transcription
  into the input word by word, with hold-to-record and a live meter.
- Persist the transcript across launches.
- The rest of the chat kit — attachments, tapbacks, reply threads, chat tabs —
  is already in the design system; see its `pill-composer` Storybook story.
