import { memo } from "react"
import { shallowEqual } from "react-redux"
import { DesktopIcon } from "../../../ui/icons"
import { selectChannel as chooseChannel } from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import {
  selectChannel,
  selectChannelActivity,
  selectContentView,
  selectPinnedIds,
  selectView,
} from "../../adapters/store/selectors"
import { StatusGlyph } from "../chrome/status-glyph"
import { ThreadRow } from "./thread-row"
import { tooltip } from "../../../ui/tooltip"

/**
 * A channel in the sidebar beside a session list: choosing it shows its
 * sessions there. It counts what waits on the person, shows when something
 * runs, and — while chosen — hangs its pinned sessions beneath it.
 */
export const ChannelRow = memo(function ChannelRow({ channelId }: { channelId: string }) {
  const dispatch = useWorkspaceDispatch()
  const channel = useWorkspaceSelector((state) => selectChannel(state, channelId))
  // Chosen while the panes are shown: over them, the Agents overview is what is chosen.
  const active = useWorkspaceSelector((state) => {
    const view = selectView(state)
    return view.channelId === channelId && selectContentView(state) === "panes"
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
    <li>
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
      />
      {pinned.length > 0 ? (
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
      ) : null}
    </li>
  )
})

/** One row of the sidebar: an icon, a label, and a count or a running glyph. */
export function SidebarRow({
  icon,
  label,
  title,
  active,
  unread,
  badge,
  badgeTone,
  running,
  onClick,
}: {
  icon: Parameters<typeof DesktopIcon>[0]["name"]
  label: string
  title?: string
  active: boolean
  unread?: boolean
  /** A count, shown instead of the running glyph. */
  badge?: number
  badgeTone?: "needs"
  running?: boolean
  onClick: () => void
}) {
  return (
    <button
      type="button"
      className="workspace-row"
      data-active={active || undefined}
      data-unread={unread || undefined}
      aria-current={active ? "page" : undefined}
      {...tooltip(title ?? "")}
      onClick={onClick}
    >
      <span className="workspace-row-icon" aria-hidden="true">
        <DesktopIcon name={icon} />
      </span>
      <span className="workspace-truncate">{label}</span>
      {badge ? (
        <span className="workspace-badge" data-tone={badgeTone}>
          {badge}
        </span>
      ) : running ? (
        <StatusGlyph status="running" />
      ) : null}
    </button>
  )
}
