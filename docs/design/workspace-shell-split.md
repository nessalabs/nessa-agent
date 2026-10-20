# Workspace shell, Settings, and command ownership — issue 280

Nothing a person sees changes. `WorkspaceShell` and `SettingsView` each
coordinate and render in one function, which
[codebase structure](../codebase-structure.md) refuses: a component renders or
coordinates, and coordination lives in a hook. `commands.ts` and its test mix
the index, the panes, and the session. This note is the split. It adds no
state.

The one command contract stays `adapters/store/commands.ts`. Callers, including
`export *` from `src/desktop/workspace/index.ts`, keep importing it. The three
files under it are the implementations, the same way `sharing-commands.ts`,
`access-commands.ts`, and `model-commands.ts` already are.

## Who owns what

```text
WorkspaceShell (ui/layouts/workspace-shell.tsx)
  calls useWorkspaceWindow, returns WorkspaceLayout
        │
        ▼
useWorkspaceWindow (ui/layouts/use-workspace-window.ts)
  review open · arrangement · rail and column fit · keys · switcher
  focus, drag, peek, escape, content mark — each a hook, composed here
        │ props only
        ▼
WorkspaceLayout (ui/layouts/workspace-layout.tsx)

SettingsHost (ui/settings-view.tsx)
  open / close / ⌘,   — unchanged public exports
        │
        ▼
useSettingsSurface (ui/use-settings-surface.ts)
  sidebar fold · chords · place (category, tab, search) · focus and scroll
        │ props only
        ▼
SettingsLayout (ui/settings-layout.tsx)

commands.ts  ── re-exports, the one list
  ├── index-commands.ts    load and follow the source
  ├── pane-commands.ts     place, move, close; ask whether it fits
  └── session-commands.ts  send, answer, stop, pin, archive, rename, delete
```

An arrow is a call or a props handoff. The layout does not dispatch, subscribe,
or read the store. A hook does not return JSX.

## State table

These are the states the window and Settings already move through. The split
keeps each row. A new state is a defect.

| Id | Where | State | Event | Next | Ordering that must hold |
| --- | --- | --- | --- | --- | --- |
| W1 | Review notice | nothing asked | `nessa-open-review` names a session | asked = that id, then try open | The listener and the store subscription share one `asked` |
| W2 | Review notice | asked, review still waits, session absent | store updates | stay asked | A session that appears later still opens |
| W3 | Review notice | asked, review no longer waits | store updates or a new notice | asked cleared, no open | A conversation that turns up later does not take a pane |
| W4 | Review notice | asked, review waits, session held | store updates | `openSession` once, asked cleared | The click is consumed |
| W5 | Arrangement | layout mounted | effect | `arrangementShown(arrangement)` | Cleanup dispatches `null` |
| W6 | Rail | preference off, item not Agents | render | item becomes Agents before paint | The reset is during render, as it is now |
| W7 | Rail | an item other than Agents is showing | any workspace key | no workspace command | The workspace root is inert |
| W8 | Switcher | closed | switcher chord | open | |
| W9 | Switcher | open | switcher chord | closed | |
| W10 | Switcher | open | `openBeside`, overview, or search chord | declined (`false`) | The chord stays consumed |
| W11 | Switcher | closed | pick a row | closed, then the open command | `canOpenBeside` is read at pick time |
| W12 | Keys | switcher open or rail item showing | a command the window owns | only `own` runs | `paused` is that condition |
| W13 | Search key | no session list | search chord | switcher opens | Focus of the list field is not asked |
| W14 | Search key | session list | search chord | list opens, then the field is focused on the next frame | |
| W15 | Columns | drag past the narrowest | edge move | the column folds by the person's choice | Width is the laid-out width at drag start, not the slide |
| S1 | Settings | closed | `openSettings(tab)` or ⌘, | open, tab remembered when named | ⌘, clears a previously named tab |
| S2 | Settings | open | close | focus returns to the opener, or `focusInFront` | The opener is read when Settings mounts |
| S3 | Settings sidebar | open | ⌘B | toggled | The listener is capture-phase and stops the window's own |
| S4 | Settings sidebar | closed | ⌘F | sidebar opens, then search focuses | Focus waits until `sidebarOpen` |
| S5 | Settings place | any | a search result chosen | category, tab, found, picked | Found clears after 1600 ms |
| S6 | Settings scroll | category or tab changes | layout | the page scrolls to the top | A found row then scrolls to center |
| S7 | Settings title | scroll | animation frame | `data-condensed` on the page, no render | The listener is removed with the category |

## Invariants

1. **One command list.** Every dispatch site imports from `commands.ts` or the
   workspace barrel. The implementation files are imported only by
   `commands.ts`. `WorkspaceCommand` stays declared in `commands.ts`, and the
   implementation files import it as a type, so they do not cycle at runtime.
   `scripts/architecture/workspace-shell-split.mjs` refuses any other import
   of those files, and `scripts/architecture/workspace-shell-split.test.mjs`
   is the regression: a direct import is a violation, an import from
   `commands.ts` is not.
2. **A layout does not decide.** `WorkspaceLayout` and `SettingsLayout` take
   props. The same check refuses them importing the workspace store hooks or
   the command list. A view that shows one fact keeps its own subscription in
   its own module (`content-mark.tsx`, `switcher-host.tsx`). Column drag
   callbacks are created in the window hook and stay referentially stable
   while their inputs do, so `Columns` still skips a render when only an
   unrelated part of the window changed.
3. **Hooks stay unconditional.** The rail's render-time reset (W6) and
   Settings' render-time sidebar refit stay in the hook that already ran the
   `useState` for that value, still during render.
4. **DOM writes that are not renders stay DOM writes.** `data-content` and
   Settings' `data-condensed` are still written on the element, not held in
   React state.
5. **No behavior change is the contract.** Existing unit tests are moved with
   the code they cover and still import the commands from `commands.ts`. A
   test that only passes against an implementation file is the wrong test.
   The overview's describes are a fourth file: they grew into the same test
   and they are not the index, the panes, or the session. Their actions stay
   the slice's, re-exported by the one list.

## Alternatives

| Alternative | Why it loses |
| --- | --- |
| One `useWorkspaceShell` with the effects inlined and the JSX still in `WorkspaceShell` | The component would still coordinate and render. That is the defect #280 names. |
| One hook file per effect (`use-review-open.ts`, `use-rail.ts`, …) | The hooks are called from one place and share the window's refs. A file per hook is a map with no second caller. Focused functions in `use-workspace-window.ts` are the split; a folder of one-caller files is not. |
| Move the window hooks to `adapters/dom/` | That folder is the page: focus, keys, measure, drag options. The review listener, the rail, and the switcher dispatch product commands and hold UI state. They stay with the layout they serve. |
| Publish `index-commands.ts` as a second import path | Two doors to one command. Callers would drift, and the barrel in `index.ts` would lie. The one list stays `commands.ts`. |
| Leave `openChannel` and `newSession` on the session module because a session is what they open | Both measure the room and change which pane shows what. The session module is what the source is asked to do to a session. Placement stays with the panes. |
| An ADR numbered 280 | The rule already lives in codebase structure. This note records the split and the states it must keep. A second record would be a second owner. |

## Out of this split

Pane insert frames (#710), the subagents panel open (#673), and the drop's
commit frame under load (#250) are another change. This split does not edit
`src/desktop/split-panes/` or the drag's timing. #281 and #669 are measurements
of the current production build, reported on the pull request; a frame that
misses its budget is not declared passed.

## Review brief

Reviewed tree: this checkout, branch `cursor/workspace-shell-split-3bfb`, base
`main` at `7b429791ec294ead4952ec86dde67d284d5aa9e9`. The brief is written
before the split. The review runs on the implementation head, dirty tree named
if any remains, and is recorded in
[the review](../reviews/workspace-shell-split.md).

**Intended behavior.** No visible change. The shell and Settings render from
props. Commands stay one list, implemented in three files. Tests that imported
`commands.ts` still do.

**Owned scope.** #280's organization. #281 and #669 only as measurements
against the 50 ms unrounded frame budget, sidebar drag first, with the results
on the pull request. A measured miss is reported, not reclassified.

**Not in scope.** #965, #981, #1038, #1039, #1030, #1034, #345, #337, #746, and
the pane-timing work #710, #673, #250. Do not edit those surfaces to make a
frame pass.

**Dimensions that apply.** Public surface and organization (module maps, the
one command import, layout components taking props). Agreement: the props the
layout renders are the same facts the hook read — sidebar open, list open,
rail, switcher, arrangement — and a contradiction (a closed sidebar with a
resize edge, a rail item showing while the workspace still takes keys) is a
finding. Representation: `data-*` attributes and focus return stay as they
are. Gates 15 and 16: the table above is the machine; no new state. Gate 17:
the desktop checks that cover the shell and Settings, plus the drag-frame
measurement.

**Dimensions that do not apply.** Gateway admission, audit sinks, Cedar
policy, provider cleanup. This change does not cross that boundary. Say so in
the review rather than skipping the agreement section.

**Known findings at the time of the brief.** None yet. The pause comments on
#280, #281, and #669 predate this assignment and are not a finding about the
split.
