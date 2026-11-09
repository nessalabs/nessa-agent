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
import {
  selectPaneClosable,
  selectPaneCount,
  selectSession,
} from "../../adapters/store/selectors"
import type { Direction, PaneKey } from "../../../split-panes/model/pane-layout"
import { useHeaderImage } from "../../../adapters/header-image"
import { DesktopIcon, type DesktopIconRole } from "../../../ui/icons"
import {
  MenuItem,
  MenuLabel,
  MenuRadioGroup,
  MenuRadioItem,
  MenuSeparator,
  MenuShortcut,
} from "../../../ui/menu"
import { useTranscriptViewPreference } from "../../adapters/storage/transcript-view-preference"
import { transcriptViews, type TranscriptView } from "../../model/transcript-view"
import { useWorkspaceFrame, type ShortcutCommand } from "../workspace-frame"

const viewIcons: Record<TranscriptView, DesktopIconRole> = {
  detailed: "transcriptDetailed",
  compact: "transcriptCompact",
}

const moves: readonly [Direction, string, ShortcutCommand, DesktopIconRole][] = [
  ["left", "Move Left", "moveLeft", "moveLeft"],
  ["right", "Move Right", "moveRight", "moveRight"],
  ["up", "Move Up", "moveUp", "moveUp"],
  ["down", "Move Down", "moveDown", "moveDown"],
]

/**
 * What a pane offers, in its "…" menu and on its header's context menu:
 * split it with a new session, open another beside it (where the layout has
 * a switcher), move it (context menu), even the panes out, show its session
 * in the sidebar, choose how much of the agent's work conversations show
 * (`transcript-view.ts`; one choice for every pane), change the header
 * picture, close it. Mounted only while a
 * menu is open: the picture is chosen through `onChooseHeaderPicture`, which
 * the pane's header holds, since the file dialog answers after the menu has
 * gone.
 */
export function PaneMenuItems({
  pane,
  sessionId,
  moves: offerMoves,
  onChooseHeaderPicture,
}: {
  pane: PaneKey
  sessionId: string
  /** Whether it offers the Move items, as the header's context menu does. */
  moves: boolean
  /** Asks for a header picture (`useChooseHeaderPicture`). */
  onChooseHeaderPicture: () => void
}) {
  const [picture, , clearPicture] = useHeaderImage()
  const [view, chooseView] = useTranscriptViewPreference()
  const dispatch = useWorkspaceDispatch()
  const frame = useWorkspaceFrame()
  const count = useWorkspaceSelector(selectPaneCount)
  const listed = useWorkspaceSelector(
    (state) => selectSession(state, sessionId) !== undefined,
  )
  const multi = count > 1
  const closable = useWorkspaceSelector((state) => selectPaneClosable(state, pane))
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
        <DesktopIcon name="splitRight" />
        Split Right
        {shortcut("splitRight")}
      </MenuItem>
      <MenuItem disabled={!fits("bottom")} onSelect={() => split("bottom")}>
        <DesktopIcon name="splitDown" />
        Split Down
        {shortcut("splitDown")}
      </MenuItem>
      <MenuItem
        onSelect={() => {
          dispatch(focusPane({ pane }))
          frame.openSwitcher(besideFits ? "split" : "open")
        }}
      >
        <DesktopIcon name={besideFits ? "openBeside" : "open"} />
        {/* With no room beside, what is picked opens here, and the item says so. */}
        {besideFits ? "Open Beside…" : "Open Here…"}
        {besideFits ? shortcut("openBeside") : null}
      </MenuItem>
      {multi && offerMoves ? (
        <>
          <MenuSeparator />
          {moves.map(([direction, label, command, icon]) => (
            <MenuItem
              key={direction}
              disabled={!dispatch(canNudge({ pane, direction }))}
              onSelect={() => dispatch(nudgePane({ pane, direction }))}
            >
              <DesktopIcon name={icon} />
              {label}
              {shortcut(command)}
            </MenuItem>
          ))}
        </>
      ) : null}
      {multi || listed ? <MenuSeparator /> : null}
      {multi ? (
        <MenuItem onSelect={() => dispatch(equalizePanes())}>
          <DesktopIcon name="evenOut" />
          Even Out Panes
        </MenuItem>
      ) : null}
      {listed ? (
        <MenuItem
          onSelect={() => {
            dispatch(revealSession({ sessionId }))
            frame.showRow(sessionId)
          }}
        >
          <DesktopIcon name="sidebar" />
          Show in Sidebar
        </MenuItem>
      ) : null}
      <MenuSeparator />
      <MenuLabel>Transcript</MenuLabel>
      <MenuRadioGroup
        value={view}
        onValueChange={(next) => {
          const chosen = transcriptViews.find((each) => each.id === next)
          if (chosen) chooseView(chosen.id)
        }}
      >
        {transcriptViews.map((each) => (
          <MenuRadioItem key={each.id} value={each.id}>
            <DesktopIcon name={viewIcons[each.id]} />
            {each.label}
          </MenuRadioItem>
        ))}
      </MenuRadioGroup>
      <MenuSeparator />
      {/* The header's own choices, from any pane, however small. */}
      <MenuItem onSelect={onChooseHeaderPicture}>
        <DesktopIcon name="picture" />
        Choose Header Picture…
      </MenuItem>
      {picture ? (
        <MenuItem onSelect={clearPicture}>
          <DesktopIcon name="nightScene" />
          Use Night Scene
        </MenuItem>
      ) : null}
      <MenuSeparator />
      <MenuItem disabled={!closable} onSelect={() => dispatch(closePane({ pane }))}>
        <DesktopIcon name="closePane" />
        Close Pane
        {shortcut("closePane")}
      </MenuItem>
    </>
  )
}
