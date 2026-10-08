import { memo, useCallback, useEffect, useState } from "react"
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuTrigger,
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from "../../../ui/menu"
import type { PaneKey } from "../../../split-panes/model/pane-layout"
import { AgentTile } from "../chrome/agent-tile"
import { IconButton } from "../../../ui/icon-button"
import { StatusGlyph } from "../chrome/status-glyph"
import { PaneMenuItems } from "./pane-menu"
import { ClosePaneButton, PaneHeaderFrame } from "./pane-header-frame"
import { useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectSession } from "../../adapters/store/selectors"
import { SessionAccessories } from "./session-accessories"
import { tooltip } from "../../../ui/tooltip"
import { useChooseHeaderPicture } from "../../../adapters/header-image"
import {
  headerImageRefusalMs,
  headerImageRefusalText,
  type HeaderImageRefusal,
} from "../../../model/header-image"

/**
 * A pane's title bar: the session's mark, title and state, what each widget
 * plugin draws for the session (`SessionAccessories`), then its "…" menu
 * and ×. At the top of a conversation the heading below already says it all,
 * so the name shows only once the heading has scrolled away; a new session's
 * home speaks for itself. With one pane the bar moves the window; with more
 * it carries the pane to another place. The × keeps its room when there is
 * nothing to close, so the menu never moves.
 */
export const PaneHeader = memo(function PaneHeader({
  pane,
  sessionId,
  multi,
  titleShown,
}: {
  pane: PaneKey
  sessionId: string
  multi: boolean
  titleShown: boolean
}) {
  // Why a picture chosen from this pane's menu was not taken, said briefly in
  // its header: the menu that asked is gone by the time the file dialog answers.
  const [refusal, setRefusal] = useState<string | null>(null)
  const choosePicture = useChooseHeaderPicture({
    onChosen: useCallback(() => setRefusal(null), []),
    onRefused: useCallback(
      (reason: HeaderImageRefusal) => setRefusal(headerImageRefusalText[reason]),
      [],
    ),
  })
  useEffect(() => {
    if (!refusal) return
    const timer = window.setTimeout(() => setRefusal(null), headerImageRefusalMs)
    return () => window.clearTimeout(timer)
  }, [refusal])
  const session = useWorkspaceSelector((state) => selectSession(state, sessionId))
  const title = session?.title ?? "New session"
  const shown = titleShown && session !== undefined
  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>
        <PaneHeaderFrame
          pane={pane}
          multi={multi}
          nameShown={shown}
          name={
            <>
              {session ? <AgentTile model={session.model} size={16} /> : null}
              <span
                className="workspace-pane-title workspace-truncate"
                {...tooltip(title)}
              >
                {title}
              </span>
              {session ? <StatusGlyph status={session.status} /> : null}
            </>
          }
          accessories={
            <>
              <SessionAccessories sessionId={sessionId} />
              {refusal ? (
                <span className="workspace-pane-refusal" role="status">
                  {refusal}
                </span>
              ) : null}
            </>
          }
          actions={
            <>
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <IconButton
                    icon="moreHorizontal"
                    label="Pane Actions"
                    draggable={false}
                  />
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end" sideOffset={6}>
                  <PaneMenuItems
                    pane={pane}
                    sessionId={sessionId}
                    moves={false}
                    onChooseHeaderPicture={choosePicture}
                  />
                </DropdownMenuContent>
              </DropdownMenu>
              <ClosePaneButton pane={pane} />
            </>
          }
        />
      </ContextMenuTrigger>
      <ContextMenuContent>
        <PaneMenuItems
          pane={pane}
          sessionId={sessionId}
          moves
          onChooseHeaderPicture={choosePicture}
        />
      </ContextMenuContent>
    </ContextMenu>
  )
})
