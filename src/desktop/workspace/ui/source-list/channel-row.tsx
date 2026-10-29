import { memo } from "react"
import { shallowEqual } from "react-redux"
import { selectChannel as chooseChannel } from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import {
  selectChannel,
  selectChannelActivity,
  selectView,
} from "../../adapters/store/selectors"
import { ChannelSettingsButton } from "../channel-settings/channel-settings"
import { SidebarRow } from "./sidebar-row"
import { useOverviewQuiet } from "./overview-quiet"

/**
 * A channel in the sidebar beside a session list: choosing it shows its
 * sessions there — its pinned ones first — so the row holds none of its own
 * beneath it. It counts what waits on the person and shows when something
 * runs.
 */
export const ChannelRow = memo(function ChannelRow({ channelId }: { channelId: string }) {
  const dispatch = useWorkspaceDispatch()
  const channel = useWorkspaceSelector((state) => selectChannel(state, channelId))
  // Chosen beside the panes. The overview paints the row quiet from
  // `data-overview-glass` (`source-list.css`) instead of re-rendering it.
  const active = useWorkspaceSelector(
    (state) => selectView(state).channelId === channelId,
  )
  // After the open paint, so the key does not render the row (`layouts.test.tsx`).
  const quiet = useOverviewQuiet()
  const activity = useWorkspaceSelector(
    (state) => selectChannelActivity(state, channelId),
    shallowEqual,
  )
  if (!channel) return null
  return (
    <SidebarRow
      icon={channel.private ? "privateChannel" : "channel"}
      label={channel.name}
      title={channel.topic}
      active={active}
      current={active && !quiet}
      unread={activity.unread}
      badge={activity.waiting}
      badgeTone="needs"
      running={activity.running}
      actions={<ChannelSettingsButton channelId={channel.id} name={channel.name} />}
      onClick={() => dispatch(chooseChannel({ channelId }))}
    />
  )
})
