/**
 * A channel's terminals in its session list, under its sessions: one row
 * each, the terminal glyph, its title and when it started, and nothing of
 * its path (that is in the pane's details). A row opens its terminal as a
 * session row opens its session, and drags to a pane the same way.
 */
import { memo, useState } from "react"
import { shallowEqual } from "react-redux"
import { GroupHeader } from "@nessa-ui/react/group-header"
import {
  closeTerminal,
  openTerminal,
  renameTerminal,
} from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import {
  selectChannelShells,
  selectTerminal,
  selectTerminalShown,
} from "../../adapters/store/selectors"
import { paneItemKey, terminalItem } from "../../model/pane-item"
import { sessionTime } from "../../model/time-labels"
import { useNow } from "../../../../clock"
import { DesktopIcon } from "../../../ui/icons"
import { IconButton } from "../../../ui/icon-button"
import { ListRow } from "../../../ui/list-row"
import { RenameField } from "../session-row-actions"

export const TerminalRows = memo(function TerminalRows({
  channelId,
}: {
  channelId: string
}) {
  const ids = useWorkspaceSelector(
    (state) => selectChannelShells(state, channelId),
    shallowEqual,
  )
  if (ids.length === 0) return null
  return (
    <div
      role="listbox"
      data-list-group="terminals"
      aria-label={`Terminals ${ids.length}`}
    >
      <GroupHeader
        aria-hidden="true"
        className="workspace-group-label"
        level={3}
        size="dense"
        tone="quiet"
        label="Terminals"
        count={ids.length}
      />
      {ids.map((id) => (
        <TerminalRow key={id} terminalId={id} />
      ))}
    </div>
  )
})

const TerminalRow = memo(function TerminalRow({ terminalId }: { terminalId: string }) {
  const dispatch = useWorkspaceDispatch()
  const terminal = useWorkspaceSelector((state) => selectTerminal(state, terminalId))
  const shown = useWorkspaceSelector((state) => selectTerminalShown(state, terminalId))
  const now = useNow(30_000)
  // The name being typed, while the person renames it here.
  const [draft, setDraft] = useState<string | null>(null)
  if (!terminal) return null
  const renaming = draft !== null
  const exited = terminal.state === "exited"
  return (
    <ListRow
      role="option"
      tabIndex={-1}
      aria-selected={shown === "focused"}
      selected={shown === "focused"}
      data-open={shown !== null || undefined}
      data-terminal-row={terminal.id}
      className="workspace-session workspace-terminal-row"
      data-drag-item={paneItemKey(terminalItem(terminal.id))}
      data-renaming={renaming || undefined}
      onClick={() => {
        if (!renaming) dispatch(openTerminal({ terminalId: terminal.id }))
      }}
      onDoubleClick={() => setDraft(terminal.title)}
      onKeyDown={(event) => {
        if (renaming || event.key !== "Enter") return
        event.preventDefault()
        dispatch(openTerminal({ terminalId: terminal.id }))
      }}
      leading={<DesktopIcon name="terminal" size={18} />}
      title={
        renaming ? (
          <RenameField
            title={terminal.title}
            draft={draft}
            label="Terminal name"
            onDraft={setDraft}
            onDone={(next) => {
              setDraft(null)
              if (next !== null && next.trim() && next.trim() !== terminal.title)
                void dispatch(renameTerminal(terminal.id, next))
            }}
          />
        ) : (
          terminal.title
        )
      }
      trailing={
        <>
          <time>
            {exited
              ? `Exited ${terminal.exitStatus ?? ""}`.trim()
              : sessionTime(terminal.startedAtMs, now)}
          </time>
          <span className="workspace-session-actions" aria-hidden="true">
            <IconButton
              icon="rename"
              label="Rename Terminal"
              size="sm"
              tabIndex={-1}
              aria-label={`Rename ${terminal.title}`}
              onClick={(event) => {
                event.stopPropagation()
                setDraft(terminal.title)
              }}
            />
            <IconButton
              icon="close"
              label="End Terminal"
              size="sm"
              tabIndex={-1}
              aria-label={`End ${terminal.title}`}
              onClick={(event) => {
                event.stopPropagation()
                void dispatch(closeTerminal(terminal.id))
              }}
            />
          </span>
        </>
      }
    />
  )
})
