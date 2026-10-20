# Local review — workspace shell split

Reviewed head: `6ba841fc602b4abdf9fdd1a547e24cdf44f8261e` on
`cursor/workspace-shell-split-3bfb`. Base: `main` at
`7b429791ec294ead4952ec86dde67d284d5aa9e9`. Working tree clean at review.
Brief: [the design note](../design/workspace-shell-split.md#review-brief).

## What was inspected

- `use-workspace-window.ts`, `workspace-layout.tsx`, `workspace-shell.tsx`,
  `content-mark.tsx`, `switcher-host.tsx`
- `use-settings-surface.ts`, `settings-layout.tsx`, `settings-view.tsx`,
  `settingsResultKey` in the catalogue
- `commands.ts` and `index-commands.ts`, `pane-commands.ts`, `session-commands.ts`
- The four command test files, still importing `./commands`
- `scripts/architecture/workspace-shell-split.mjs` and its test
- Module maps in the workspace and settings barrels, codebase structure,
  architecture, and the state atlas sources that named the old files

Not inspected as product behavior: gateway admission, audit, Cedar, provider
cleanup. This change does not cross that boundary.

## Agreement

The layout renders the hook's facts and no others.

| Fact | Owner | Where it is drawn |
| --- | --- | --- |
| Sidebar drawn | `selectSidebarOpen` in `useWorkspaceChrome` | `data-sidebar`, the resize edge, the peek |
| Sidebar chosen | `chrome.sidebar.open`, the person's choice, not the fold | `data-sidebar-chosen` |
| Session list drawn | `region.sessionList && selectSessionListOpen` | `data-list`, the list edge |
| Listed channel | that same list flag and `selectView` | `ListedChannelProvider` |
| Rail item showing | the rail preference and the item, computed once | `inert`, `data-rail-view`, and keys paused |
| Arrangement | `useShownArrangement` | dispatched, not drawn |
| Settings sidebar | `drawn(sidebarColumn)` in `useSettingsSurface` | `data-sidebar`, peek, chords |

A closed sidebar cannot draw its resize edge: the edge is `sidebarOpen ?`.
A rail item showing pauses workspace keys and sets `inert` from the same
`shownRailItem`. No second flag.

## Dimensions

Public surface and organization. Callers of `WorkspaceShell`, `SidebarRegion`,
`openSettings`, `SettingsHost`, and `useSettingsOpening` are unchanged.
Commands are imported from `commands.ts`. The architecture check refuses a
layout importing the store hooks or the command list, and refuses any file
but `commands.ts` importing the three implementation modules.
`scripts/architecture/workspace-shell-split.test.mjs` is the regression.

Authority, admission, failure and cleanup, observations, audit, and resource
bounds do not apply: no new dispatch authority, no gateway call, no new
retained text.

Representation. `data-content` and Settings' `data-condensed` are still
written on the element. Focus still returns to the opener. The tab pill is
still a measured style, not React state.

## Gates 15, 16, 17

The state table in the design note was not extended. No new state, event, or
ordering. No new behavior. Gate 17's browser numbers are not in this review;
they are the drag-frame measurement that follows on this head, reported on
the pull request. A miss stays a miss.

## Revert probe

Appending `import { useWorkspaceDispatch } from "../../adapters/store/hooks"`
to `workspace-layout.tsx` made `node scripts/check-architecture.mjs` fail
with `workspace-layout renders from props and does not import .../hooks`.
The line was removed. The same command then passed. The unit test covers the
same refusal without editing the layout.

## Findings

None. The tab strip's arrow keys stay on the strip: they are that control's
keys, not the window chords the hook took. `ready`, `shown`, and `outboxOf`
live on the workspace testing module because a file that is not a test cannot
import it.
