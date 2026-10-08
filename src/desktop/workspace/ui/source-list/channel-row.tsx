import { memo } from "react"
import { shallowEqual } from "react-redux"
import { DesktopIcon } from "../../../ui/icons"
import { selectChannel as chooseChannel } from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import {
  selectChannel,
  selectChannelActivity,
  selectPinnedIds,
  selectView,
} from "../../adapters/store/selectors"
import { StatusGlyph } from "../chrome/status-glyph"
import { ThreadRow } from "./thread-row"
import { useOverviewQuiet } from "./overview-quiet"
import { tooltip } from "../../../ui/tooltip"

/**
 * A channel in the sidebar beside a session list: choosing it shows its
 * sessions there. It counts what waits on the person, shows when something
 * runs, and — while chosen — hangs its pinned sessions beneath it.
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
  const pinned = useWorkspaceSelector(
    (state) =>
      selectView(state).channelId === channelId ? selectPinnedIds(state, channelId) : [],
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
        current={active && !quiet}
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
  shortcut,
  className,
  active,
  current,
  unread,
  badge,
  badgeTone,
  badgeLabel,
  running,
  onClick,
}: {
  icon: Parameters<typeof DesktopIcon>[0]["name"]
  label: string
  title?: string
  /** The chord that does the same, drawn quieter in the tooltip. */
  shortcut?: string
  /** A hook for a surface that styles or finds its one row. */
  className?: string
  active: boolean
  /** The sidebar's current page. Lags `active` while the overview opens. */
  current: boolean
  unread?: boolean
  /** A count, shown instead of the running glyph. */
  badge?: number
  badgeTone?: "needs"
  /** What the count means, for a reader who cannot see its colour. */
  badgeLabel?: string
  running?: boolean
  onClick: () => void
}) {
  return (
    <button
      type="button"
      className={className ? `workspace-row ${className}` : "workspace-row"}
      data-active={active || undefined}
      data-unread={unread || undefined}
      aria-current={current ? "page" : undefined}
      {...tooltip(title ?? "", { shortcut })}
      onClick={onClick}
    >
      <span className="workspace-row-icon" aria-hidden="true">
        <DesktopIcon name={icon} />
      </span>
      <span className="workspace-truncate">{label}</span>
      {badge ? (
        <span className="workspace-badge" data-tone={badgeTone} aria-label={badgeLabel}>
          {badge}
        </span>
      ) : running ? (
        <StatusGlyph status="running" />
      ) : null}
    </button>
  )
}
