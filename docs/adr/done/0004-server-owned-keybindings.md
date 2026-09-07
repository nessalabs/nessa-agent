# 0004. Client shortcuts are server-authored; `shortcuts.json` is a cache

- **Date:** 2026-09-03
- **Status:** accepted

## Context

The panel needs **tab shortcuts** (new tab, close tab, go to the first / second
/ … tab) when the floating window is focused — without fighting the browser
under `just web`, and without registering tab actions as OS-global shortcuts
(other Nessa surfaces will exist later).

A spike put those shortcuts in TypeScript. That cannot ship: defaults fork per
surface, org-wide changes need an app release, and the host already keeps a
separate `toggleShortcut` in settings. Two authorities will multiply every time
we add a surface.

We want this **early**. If we wait until there are more surfaces, we will
hardcode again and forget to centralize.

Constraint: **one vocabulary of actions**, shortcut keys as data, **server as
runtime authority**, a local **`shortcuts.json`** so cold start and pre-connect
panel summon still work. Where that file sits on disk is
[0005](0005-stage-scoped-local-data.md) (stage-scoped roots; prod bare).

## Decision

Shortcuts are a typed document with a dedicated on-disk file.

1. **Defaults** live in the repo as `protocol/defaults/shortcuts.v1.json`. The
   **server** loads and serves that document on connect (`HelloOk`). The shell
   never invents default keys in code.
2. **Local cache:** the host writes the same shape to **`shortcuts.json`** under
   the stage-scoped config dir from 0005 (alongside geometry `settings.json`,
   not inside it). Seed from the defaults file when absent; replace from HelloOk
   after connect.
3. **Matching:** the shell matches focused keydown against the active document
   (cache, then HelloOk refresh). Product code dispatches **action ids**, not
   `Cmd+T` / `Cmd+1` literals.
4. **Summon** (`panel.summon`) lives in the same document with scope `global`.
   Drop the host’s lone `toggleShortcut` field; register/re-register from
   `shortcuts.json` after refresh.

**Actions:**

| Action | Meaning |
| --- | --- |
| `panel.summon` | Show/hide the panel (`global`) |
| `panel.newTab` | Open a conversation tab |
| `panel.closeTab` | Close the active tab |
| `panel.activateTab` | Switch to a tab; args select which |

Each row: `keys`, `action`, optional `args`, `scope` (`global` | `focused`),
`surface` (`desktop` | `browser` | `*`).

**v1 defaults:**

| Action | Desktop (focused) | Browser (focused) |
| --- | --- | --- |
| `panel.newTab` | `CmdOrCtrl+T`, `CmdOrCtrl+N` | `CmdOrCtrl+Shift+T` |
| `panel.closeTab` | `CmdOrCtrl+W` | `CmdOrCtrl+Shift+W` |
| `panel.activateTab` `{ "index": 0…8 }` | `CmdOrCtrl+1`…`9` | `CmdOrCtrl+Shift+1`…`9` |
| `panel.summon` | `CmdOrCtrl+Shift+D` (global) | — |

Index `0` is the first open tab. Too few tabs → no-op. Ignore key-repeat.

**Args now, so we do not forget later:** v1 uses `{ "index": N }`. Later
settings may use `{ "conversationId": "…" }` so e.g. Cmd+1 always goes to a
specific tab. Resolve: `conversationId` if present and open, else `index`.

**v1: no personal overrides.** Changing shortcuts means changing server
defaults (or a future settings API). `shortcuts.json` is a cache, not a second
source of truth.

Supersede the hardcoded matcher in `src/panel/adapters/tab-shortcuts.ts`.

## Alternatives considered

- **Hardcode shortcuts in the shell.** Loses server authority; forks desktop vs
  browser in `if`s. Rejected.
- **Bury shortcuts inside `settings.json` with panel geometry.** Mixes host
  chrome preferences with the action catalog we will grow. Rejected in favor of
  a dedicated `shortcuts.json`.
- **Local file as personal overrides in v1.** Needs a sync/merge story we do
  not have yet. Rejected for v1; schema still allows richer `args`.
- **OS-global shortcuts for tab actions.** Collides with other apps and future
  surfaces. Rejected; only `panel.summon` is global.

## Consequences

Easier: one defaults file; browser-safe tab shortcuts are data; index and
later id-based activate share one action; summon shares the table; tests use
fixtures.

Harder: HelloOk and `shortcuts.json` must stay aligned; bad key strings skip +
log, never block launch; host re-registers summon after refresh.

Watch for: literal shortcut keys creeping back into the shell; treating
`shortcuts.json` as user truth without a sync story.

## Implementation sketch (approving PR)

Depends on [0005](0005-stage-scoped-local-data.md) for the config root.

1. `protocol/defaults/shortcuts.v1.json` + schema on `HelloOk` / fixtures +
   `pnpm protocol:generate`.
2. Server serves that document on connect (default summon `CmdOrCtrl+Shift+D`).
3. Host: read/write stage-scoped `shortcuts.json`, seed from defaults, refresh
   summon.
4. Shell: hydrate → HelloOk → config-driven focused matcher (`newTab` /
   `closeTab` / `activateTab`).
5. README: `shortcuts.json` + pointer to 0005 for paths; this ADR is the
   shortcuts authority model.

**Out of scope for that PR:** settings UI, per-user override merge, mid-session
push without reconnect, reopening a closed tab from a `conversationId` binding.
