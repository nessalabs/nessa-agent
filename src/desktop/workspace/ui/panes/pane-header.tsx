import { memo } from "react"
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuTrigger,
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from "../../../ui/menu"
import { closePane } from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectSession } from "../../adapters/store/selectors"
import type { PaneKey } from "../../model/pane-layout"
import { AgentTile } from "../chrome/agent-tile"
import { IconButton } from "../chrome/icon-button"
import { StatusGlyph } from "../chrome/status-glyph"
import { useWorkspaceFrame } from "../workspace-frame"
import { PaneMenuItems } from "./pane-menu"
import { tooltip } from "../../../ui/tooltip"

/**
 * A pane's title bar: the session's mark, title and state, then its "…" menu
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
  const dispatch = useWorkspaceDispatch()
  const frame = useWorkspaceFrame()
  const session = useWorkspaceSelector((state) => selectSession(state, sessionId))
  const title = session?.title ?? "New session"
  const shown = titleShown && session !== undefined
  // The last pane closes back to a new session's home; a home itself has nothing to close.
  const closable = multi || session !== undefined
  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>
        <header
          className="workspace-pane-header"
          data-tauri-drag-region={multi ? undefined : true}
          // With more than one pane, the bar carries the pane (`adapters/dom/drag.ts`).
          data-drag-pane={multi ? pane : undefined}
        >
          <div
            className="workspace-pane-name"
            data-shown={shown || undefined}
            aria-hidden={!shown}
          >
            {session ? <AgentTile model={session.model} size={16} /> : null}
            <span className="workspace-pane-title workspace-truncate" {...tooltip(title)}>
              {title}
            </span>
            {session ? <StatusGlyph status={session.status} /> : null}
          </div>
          {/* With one pane the whole bar moves the window; with more, none of it
              may, or pressing it to carry the pane would carry the window. */}
          <span
            className="workspace-spacer"
            data-tauri-drag-region={multi ? undefined : true}
          />
          <div className="workspace-pane-actions">
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <IconButton
                  icon="moreHorizontal"
                  label="Pane Actions"
                  draggable={false}
                />
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end" sideOffset={6}>
                <PaneMenuItems pane={pane} sessionId={sessionId} moves={false} />
              </DropdownMenuContent>
            </DropdownMenu>
            <IconButton
              icon="close"
              label="Close Pane"
              shortcut={frame.shortcut("closePane")}
              draggable={false}
              data-reserved={!closable || undefined}
              tabIndex={closable ? 0 : -1}
              aria-hidden={!closable || undefined}
              onClick={(event) => {
                event.stopPropagation()
                if (closable) dispatch(closePane({ pane }))
              }}
            />
          </div>
        </header>
      </ContextMenuTrigger>
      <ContextMenuContent>
        <PaneMenuItems pane={pane} sessionId={sessionId} moves />
      </ContextMenuContent>
    </ContextMenu>
  )
})
