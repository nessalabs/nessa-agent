import { ChannelSettingsButton } from "../channel-settings/channel-settings"
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
  listPaintFrame,
  usePaintedWithPanes,
} from "../../adapters/store/paint-with-panes"
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
import { StatusGlyph } from "../chrome/status-glyph"
import { WorkingWords } from "../chrome/working-words"
import { ThreadRow } from "./thread-row"
import "./channel-branch.css"
import { NewInChannelButton } from "../terminals/new-menu"
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
  const current = usePaintedWithPanes(
    (state) => selectFocusedChannel(state) === channelId,
    listPaintFrame,
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
      // Folded, it still says that something waits, in the slot its + takes
      // under the pointer; that something runs, its name shimmers.
      badge={
        summary && activity.waiting > 0 ? (
          <StatusGlyph status="needs-you" decorative />
        ) : undefined
      }
      trailing={
        <>
          <ChannelSettingsButton channelId={channel.id} name={channel.name} />
          <NewInChannelButton channelId={channel.id} label={label} tabIndex={-1} />
        </>
      }
      showTrailingOnHover
      submenu={expanded ? <BranchSessions channelId={channel.id} /> : undefined}
    >
      <WorkingWords working={summary !== null && activity.waiting === 0}>
        {channel.name}
      </WorkingWords>
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
        <ThreadRow key={sessionId} sessionId={sessionId} channelId={channelId} />
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
