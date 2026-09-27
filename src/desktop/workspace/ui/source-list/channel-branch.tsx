import { memo } from "react"
import { shallowEqual } from "react-redux"
import { DesktopIcon } from "../../../ui/icons"
import {
  newSession,
  openChannel,
  toggleChannel,
  toggleShowAll,
} from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import {
  sameBranch,
  selectBranch,
  selectChannel,
  selectChannelActivity,
  selectChannelExpanded,
  selectFocusedChannel,
  selectShowAll,
} from "../../adapters/store/selectors"
import { commandKey } from "../../adapters/dom/shortcuts"
import { branchCap } from "../../model/session-groups"
import { IconButton } from "../chrome/icon-button"
import { StatusGlyph } from "../chrome/status-glyph"
import { useFocusedRoom } from "../session-actions"
import { ThreadRow } from "./thread-row"
import "./channel-branch.css"

/**
 * A channel in the sidebar that lists sessions inline. Choosing it opens its
 * newest session (⌘ beside); its glyph turns into a disclosure under the
 * pointer, which shows the few sessions that matter — the newest, any on
 * screen, any waiting — with "Show all" for the rest. Folded, it still says
 * whether something waits or runs.
 */
export const ChannelBranch = memo(function ChannelBranch({
  channelId,
}: {
  channelId: string
}) {
  const dispatch = useWorkspaceDispatch()
  const room = useFocusedRoom()
  const channel = useWorkspaceSelector((state) => selectChannel(state, channelId))
  const expanded = useWorkspaceSelector((state) =>
    selectChannelExpanded(state, channelId),
  )
  const current = useWorkspaceSelector(
    (state) => selectFocusedChannel(state) === channelId,
  )
  const activity = useWorkspaceSelector(
    (state) => selectChannelActivity(state, channelId),
    shallowEqual,
  )
  if (!channel) return null
  const label = `New session in #${channel.name}`
  return (
    <li className="workspace-branch">
      <div className="workspace-branch-head">
        <button
          type="button"
          className="workspace-row"
          data-row="channel"
          data-channel={channel.id}
          data-current={current || undefined}
          data-unread={activity.unread || undefined}
          aria-expanded={expanded}
          title={channel.topic}
          onClick={(event) => {
            const beside = commandKey(event)
            dispatch(
              openChannel({
                channelId: channel.id,
                beside,
                room: beside ? room() : undefined,
              }),
            )
          }}
        >
          <span
            className="workspace-disclosure"
            aria-hidden="true"
            onClick={(event) => {
              event.stopPropagation()
              dispatch(toggleChannel({ channelId: channel.id }))
            }}
          >
            <DesktopIcon name={channel.private ? "privateChannel" : "channel"} />
            <DesktopIcon name="chevronRight" />
          </span>
          <span className="workspace-truncate">{channel.name}</span>
        </button>
        {!expanded && (activity.waiting > 0 || activity.running) ? (
          <span className="workspace-branch-summary">
            <StatusGlyph status={activity.waiting > 0 ? "needs-you" : "running"} />
          </span>
        ) : null}
        <IconButton
          className="workspace-branch-add"
          icon="add"
          label={label}
          tabIndex={-1}
          onClick={() => dispatch(newSession({ channelId: channel.id }))}
        />
      </div>
      {expanded ? <BranchSessions channelId={channel.id} /> : null}
    </li>
  )
})

function BranchSessions({ channelId }: { channelId: string }) {
  const dispatch = useWorkspaceDispatch()
  const branch = useWorkspaceSelector(
    (state) => selectBranch(state, channelId),
    sameBranch,
  )
  const showAll = useWorkspaceSelector((state) => selectShowAll(state, channelId))
  return (
    <ul className="workspace-branch-sessions workspace-rise">
      {branch.ids.map((sessionId) => (
        <ThreadRow
          key={sessionId}
          sessionId={sessionId}
          channelId={channelId}
          kind="branch"
        />
      ))}
      {branch.hidden > 0 || (showAll && branch.total > branchCap) ? (
        <li>
          <button
            type="button"
            className="workspace-row workspace-more"
            data-row="more"
            data-parent={channelId}
            onClick={() => dispatch(toggleShowAll({ channelId }))}
          >
            {showAll ? "Show fewer" : `Show all ${branch.total}`}
          </button>
        </li>
      ) : null}
      {branch.total === 0 ? (
        <li>
          <button
            type="button"
            className="workspace-row workspace-more"
            data-row="more"
            data-parent={channelId}
            onClick={() => dispatch(newSession({ channelId }))}
          >
            Start a session
          </button>
        </li>
      ) : null}
    </ul>
  )
}
