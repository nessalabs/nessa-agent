import { memo } from "react"
import { shallowEqual } from "react-redux"
import { selectChannel as chooseChannel } from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import {
  selectChannel,
  selectChannelActivity,
  selectOverviewOpen,
  selectPinnedIds,
  selectView,
} from "../../adapters/store/selectors"
import { ThreadRow } from "./thread-row"
import { SidebarRow } from "./sidebar-row"

/**
 * A channel in the sidebar beside a session list: choosing it shows its
 * sessions there. It counts what waits on the person, shows when something
 * runs, and — while chosen — hangs its pinned sessions beneath it.
 */
export const ChannelRow = memo(function ChannelRow({ channelId }: { channelId: string }) {
  const dispatch = useWorkspaceDispatch()
  const channel = useWorkspaceSelector((state) => selectChannel(state, channelId))
  // Chosen beside the panes or a widget over them: over both, the Agents
  // overview is what is chosen.
  const active = useWorkspaceSelector((state) => {
    const view = selectView(state)
    return view.channelId === channelId && !selectOverviewOpen(state)
  })
  const activity = useWorkspaceSelector(
    (state) => selectChannelActivity(state, channelId),
    shallowEqual,
  )
  const pinned = useWorkspaceSelector(
    (state) => (active ? selectPinnedIds(state, channelId) : []),
    shallowEqual,
  )
  if (!channel) return null
  return (
    <SidebarRow
      icon={channel.private ? "privateChannel" : "channel"}
      label={channel.name}
      title={channel.topic}
      active={active}
      unread={activity.unread}
      badge={activity.waiting}
      badgeTone="needs"
      running={activity.running}
      onClick={() => dispatch(chooseChannel({ channelId }))}
      submenu={
        pinned.length > 0 ? (
          <ul role="list" className="workspace-thread workspace-rise">
            {pinned.map((sessionId) => (
              <ThreadRow
                key={sessionId}
                sessionId={sessionId}
                channelId={channelId}
                kind="pinned"
              />
            ))}
          </ul>
        ) : undefined
      }
    />
  )
})
