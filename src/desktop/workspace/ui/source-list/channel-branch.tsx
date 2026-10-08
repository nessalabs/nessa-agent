import { memo } from "react"
import { SidebarMenuItem } from "@nessa-ui/react/sidebar"
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
import { useBesideKey } from "../session-actions"
import { branchCap, statusLabels } from "../../model/session-groups"
import { IconButton } from "../../../ui/icon-button"
import { StatusGlyph } from "../chrome/status-glyph"
import { ThreadRow } from "./thread-row"
import "./channel-branch.css"
import { tooltip } from "../../../ui/tooltip"

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
  const besideKey = useBesideKey()
  const dispatch = useWorkspaceDispatch()
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
  // Folded, what waits or runs is said with the row's name: the kit lays it beside the control.
  const summary =
    !expanded && (activity.waiting > 0 || activity.running)
      ? statusLabels[activity.waiting > 0 ? "needs-you" : "running"]
      : null
  return (
    <SidebarMenuItem
      size="xs"
      containerClassName="workspace-branch"
      data-row="channel"
      data-channel={channel.id}
      data-current={current || undefined}
      unread={activity.unread}
      aria-expanded={expanded}
      aria-label={summary ? `${channel.name} ${summary}` : undefined}
      {...tooltip(channel.topic)}
      onClick={(event) => {
        const beside = besideKey.asks(event)
        dispatch(
          openChannel({
            channelId: channel.id,
            beside,
          }),
        )
      }}
      icon={
        <span
          className="workspace-disclosure"
          onClick={(event) => {
            event.stopPropagation()
            dispatch(toggleChannel({ channelId: channel.id }))
          }}
        >
          <DesktopIcon name={channel.private ? "privateChannel" : "channel"} />
          <DesktopIcon name="chevronRight" />
        </span>
      }
      // Folded, it still says whether something waits or runs, in the slot
      // its + takes under the pointer.
      badge={
        summary ? (
          <StatusGlyph
            status={activity.waiting > 0 ? "needs-you" : "running"}
            decorative
          />
        ) : undefined
      }
      trailing={
        <IconButton
          icon="add"
          label={label}
          tabIndex={-1}
          onClick={() => dispatch(newSession({ channelId: channel.id }))}
        />
      }
      showTrailingOnHover
      submenu={expanded ? <BranchSessions channelId={channel.id} /> : undefined}
    >
      {channel.name}
    </SidebarMenuItem>
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
        <SidebarMenuItem
          size="xs"
          className="workspace-more"
          data-row="more"
          data-parent={channelId}
          onClick={() => dispatch(toggleShowAll({ channelId }))}
        >
          {showAll ? "Show fewer" : `Show all ${branch.total}`}
        </SidebarMenuItem>
      ) : null}
      {branch.total === 0 ? (
        <SidebarMenuItem
          size="xs"
          className="workspace-more"
          data-row="more"
          data-parent={channelId}
          onClick={() => dispatch(newSession({ channelId }))}
        >
          Start a session
        </SidebarMenuItem>
      ) : null}
    </ul>
  )
}
