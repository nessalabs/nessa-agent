import {
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuShortcut,
} from "@nessa-ui/react/context-menu"
import {
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
} from "@nessa-ui/react/dropdown-menu"
import type { ReactNode } from "react"
import {
  closePane,
  equalizePanes,
  focusPane,
  newSession,
  nudgePane,
  revealSession,
} from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import {
  selectPaneCount,
  selectPanes,
  selectSession,
} from "../../adapters/store/selectors"
import { isFull, type Direction, type PaneKey } from "../../model/pane-layout"
import { useWorkspaceFrame, type ShortcutCommand } from "../workspace-frame"

const menus = {
  dropdown: {
    Item: DropdownMenuItem,
    Separator: DropdownMenuSeparator,
    Shortcut: DropdownMenuShortcut,
  },
  context: {
    Item: ContextMenuItem,
    Separator: ContextMenuSeparator,
    Shortcut: ContextMenuShortcut,
  },
}

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
  kind,
  pane,
  sessionId,
}: {
  kind: keyof typeof menus
  pane: PaneKey
  sessionId: string
}) {
  const { Item, Separator, Shortcut } = menus[kind]
  const dispatch = useWorkspaceDispatch()
  const frame = useWorkspaceFrame()
  const count = useWorkspaceSelector(selectPaneCount)
  const full = useWorkspaceSelector((state) => {
    const panes = selectPanes(state)
    return panes === null || isFull(panes)
  })
  const listed = useWorkspaceSelector(
    (state) => selectSession(state, sessionId) !== undefined,
  )
  const multi = count > 1
  const closable = multi || listed
  const shortcut = (command: ShortcutCommand): ReactNode => {
    const label = frame.shortcut(command)
    return label ? <Shortcut>{label}</Shortcut> : null
  }
  const split = (beside: "right" | "bottom") =>
    dispatch(newSession({ beside, target: pane, room: frame.roomOf(pane) }))
  return (
    <>
      <Item disabled={full} onSelect={() => split("right")}>
        Split Right
        {shortcut("splitRight")}
      </Item>
      <Item disabled={full} onSelect={() => split("bottom")}>
        Split Down
        {shortcut("splitDown")}
      </Item>
      {frame.openSwitcher ? (
        <Item
          onSelect={() => {
            dispatch(focusPane({ pane }))
            frame.openSwitcher?.("split")
          }}
        >
          Open Beside…
          {shortcut("openBeside")}
        </Item>
      ) : null}
      {multi && kind === "context" ? (
        <>
          <Separator />
          {moves.map(([direction, label, command]) => (
            <Item
              key={direction}
              onSelect={() => dispatch(nudgePane({ pane, direction }))}
            >
              {label}
              {shortcut(command)}
            </Item>
          ))}
        </>
      ) : null}
      {multi || listed ? <Separator /> : null}
      {multi ? (
        <Item onSelect={() => dispatch(equalizePanes())}>Even Out Panes</Item>
      ) : null}
      {listed ? (
        <Item
          onSelect={() => {
            dispatch(revealSession({ sessionId }))
            frame.showRow(sessionId)
          }}
        >
          Show in Sidebar
        </Item>
      ) : null}
      <Separator />
      <Item disabled={!closable} onSelect={() => dispatch(closePane({ pane }))}>
        Close Pane
        {shortcut("closePane")}
      </Item>
    </>
  )
}
