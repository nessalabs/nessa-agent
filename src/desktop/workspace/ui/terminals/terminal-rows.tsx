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
import {
  effectiveTerminalStatus,
  terminalAgentOf,
  terminalStatusLine,
} from "../../model/terminal"
import { sessionTime } from "../../model/time-labels"
import { useNow } from "../../../../clock"
import { IconButton } from "../../../ui/icon-button"
import { ListRow } from "../../../ui/list-row"
import { WorkingWords } from "../chrome/working-words"
import { RenameField } from "../session-row-actions"
import {
  TerminalIcon,
  TerminalStatusIndicator,
  terminalStatusSummary,
} from "./terminal-icon"

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
  // The name being typed, while the person renames it here.
  const [draft, setDraft] = useState<string | null>(null)
  if (!terminal) return null
  const renaming = draft !== null
  const exited = terminal.state === "exited"
  const agent = terminalAgentOf(terminal)
  const status = effectiveTerminalStatus(terminal)
  const working = status?.state === "working"
  const activeStatus =
    status && status.state !== "idle" ? terminalStatusLine(status) : undefined
  const label = terminalStatusSummary(agent, status)
  return (
    <ListRow
      role="option"
      tabIndex={-1}
      aria-selected={shown === "focused"}
      aria-label={label ? `${terminal.title}. ${label}.` : undefined}
      selected={shown === "focused"}
      unread={status?.state === "done"}
      data-open={shown !== null || undefined}
      data-terminal-row={terminal.id}
      data-terminal-agent={agent ?? undefined}
      data-program-state={status?.state}
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
      leading={<TerminalIcon agent={agent} size={18} />}
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
          <WorkingWords working={working} spoken>
            {terminal.title}
          </WorkingWords>
        )
      }
      description={renaming ? undefined : activeStatus}
      trailing={
        <>
          {status && status.state !== "idle" ? (
            <TerminalStatusIndicator status={status} />
          ) : exited ? (
            <time>{`Exited ${terminal.exitStatus ?? ""}`.trim()}</time>
          ) : (
            <TerminalTime at={terminal.startedAtMs} as="time" />
          )}
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

/**
 * A channel's open shells rendered inline beneath the channel in the sidebar
 * tree, showing the Claude/Codex badge and live OSC 7501 status.
 */
export const TerminalBranchRows = memo(function TerminalBranchRows({
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
    <>
      {ids.map((id) => (
        <TerminalBranchRow key={id} terminalId={id} channelId={channelId} />
      ))}
    </>
  )
})

const TerminalBranchRow = memo(function TerminalBranchRow({
  terminalId,
  channelId,
}: {
  terminalId: string
  channelId: string
}) {
  const dispatch = useWorkspaceDispatch()
  const terminal = useWorkspaceSelector((state) => selectTerminal(state, terminalId))
  const shown = useWorkspaceSelector((state) => selectTerminalShown(state, terminalId))
  if (!terminal) return null
  const exited = terminal.state === "exited"
  const agent = terminalAgentOf(terminal)
  const status = effectiveTerminalStatus(terminal)
  const working = status?.state === "working"
  const label = terminalStatusSummary(agent, status)
  return (
    <li>
      <ListRow
        as="button"
        className="workspace-thread-row workspace-terminal-branch-row"
        data-row="terminal"
        data-terminal-row={terminal.id}
        data-terminal-agent={agent ?? undefined}
        data-program-state={status?.state}
        data-parent={channelId}
        data-open={shown !== null || undefined}
        selected={shown === "focused"}
        unread={status?.state === "done"}
        aria-label={label ? `${terminal.title}. ${label}.` : terminal.title}
        data-drag-item={paneItemKey(terminalItem(terminal.id))}
        onClick={() => dispatch(openTerminal({ terminalId: terminal.id }))}
        leading={<TerminalIcon agent={agent} size={16} />}
        title={
          <WorkingWords working={working} spoken>
            {terminal.title}
          </WorkingWords>
        }
        trailing={
          status && status.state !== "idle" ? (
            <TerminalStatusIndicator status={status} />
          ) : exited ? (
            <span className="workspace-time">
              {`Exited ${terminal.exitStatus ?? ""}`.trim()}
            </span>
          ) : (
            <TerminalTime at={terminal.startedAtMs} as="span" />
          )
        }
      />
    </li>
  )
})

function TerminalTime({ at, as }: { at: number; as: "time" | "span" }) {
  const now = useNow(30_000)
  const label = sessionTime(at, now)
  return as === "time" ? (
    <time>{label}</time>
  ) : (
    <span className="workspace-time">{label}</span>
  )
}
