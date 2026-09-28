import type { ReactNode } from "react"
import {
  canNudge,
  canOpenBeside,
  closePane,
  equalizePanes,
  focusPane,
  newSession,
  nudgePane,
  revealSession,
} from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectPaneCount, selectSession } from "../../adapters/store/selectors"
import type { Direction, PaneKey } from "../../model/pane-layout"
import { MenuItem, MenuSeparator, MenuShortcut } from "../../../ui/menu"
import { useWorkspaceFrame, type ShortcutCommand } from "../workspace-frame"

const moves: readonly [Direction, string, ShortcutCommand][] = [
  ["left", "Move Left", "moveLeft"],
  ["right", "Move Right", "moveRight"],
  ["up", "Move Up", "moveUp"],
  ["down", "Move Down", "moveDown"],
]

/**
 * What a pane offers, in its "…" menu and on its header's context menu:
 * split it with a new session, open another beside it (where the layout has
 * a switcher), move it (context menu), even the panes out, show its session
 * in the sidebar, close it. Mounted only while a menu is open.
 */
export function PaneMenuItems({
  pane,
  sessionId,
  moves: offerMoves,
}: {
  pane: PaneKey
  sessionId: string
  /** Whether it offers the Move items, as the header's context menu does. */
  moves: boolean
}) {
  const dispatch = useWorkspaceDispatch()
  const frame = useWorkspaceFrame()
  const count = useWorkspaceSelector(selectPaneCount)
  const listed = useWorkspaceSelector(
    (state) => selectSession(state, sessionId) !== undefined,
  )
  const multi = count > 1
  const closable = multi || listed
  const shortcut = (command: ShortcutCommand): ReactNode => {
    const label = frame.shortcut(command)
    return label ? <MenuShortcut>{label}</MenuShortcut> : null
  }
  // Asked of the one rule the commands follow, as the menu opens: an item is
  // offered only where it will do what it says.
  const fits = (side: "right" | "bottom") =>
    dispatch(canOpenBeside({ target: pane, side }))
  const besideFits = dispatch(canOpenBeside({ target: pane }))
  const split = (beside: "right" | "bottom") =>
    dispatch(newSession({ beside, target: pane }))
  return (
    <>
      <MenuItem disabled={!fits("right")} onSelect={() => split("right")}>
        Split Right
        {shortcut("splitRight")}
      </MenuItem>
      <MenuItem disabled={!fits("bottom")} onSelect={() => split("bottom")}>
        Split Down
        {shortcut("splitDown")}
      </MenuItem>
      <MenuItem
        onSelect={() => {
          dispatch(focusPane({ pane }))
          frame.openSwitcher(besideFits ? "split" : "open")
        }}
      >
        {/* With no room beside, what is picked opens here, and the item says so. */}
        {besideFits ? "Open Beside…" : "Open Here…"}
        {besideFits ? shortcut("openBeside") : null}
      </MenuItem>
      {multi && offerMoves ? (
        <>
          <MenuSeparator />
          {moves.map(([direction, label, command]) => (
            <MenuItem
              key={direction}
              disabled={!dispatch(canNudge({ pane, direction }))}
              onSelect={() => dispatch(nudgePane({ pane, direction }))}
            >
              {label}
              {shortcut(command)}
            </MenuItem>
          ))}
        </>
      ) : null}
      {multi || listed ? <MenuSeparator /> : null}
      {multi ? (
        <MenuItem onSelect={() => dispatch(equalizePanes())}>Even Out Panes</MenuItem>
      ) : null}
      {listed ? (
        <MenuItem
          onSelect={() => {
            dispatch(revealSession({ sessionId }))
            frame.showRow(sessionId)
          }}
        >
          Show in Sidebar
        </MenuItem>
      ) : null}
      <MenuSeparator />
      <MenuItem disabled={!closable} onSelect={() => dispatch(closePane({ pane }))}>
        Close Pane
        {shortcut("closePane")}
      </MenuItem>
    </>
  )
}
