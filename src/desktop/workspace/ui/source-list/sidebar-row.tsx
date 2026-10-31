import type { ReactNode } from "react"
import { Badge } from "@nessa-ui/react/badge"
import { SidebarMenuItem } from "@nessa-ui/react/sidebar"
import { DesktopIcon } from "../../../ui/icons"
import { tooltip } from "../../../ui/tooltip"
import { statusLabels } from "../../model/session-groups"

/**
 * One row of the sidebar: the kit's `SidebarMenuItem`, dense, with an icon, a
 * label, and a count at its end. While something runs its icon breathes in the
 * running light, so the row's end holds no spinner; the row's actions show
 * only under the pointer or keyboard focus, in the count's place. The
 * sidebar's skin (`source-list.css` › Rows) gives it the window's inks.
 */
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
  submenu,
  actions,
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
  /** A count at the row's end; its actions take its place under the pointer. */
  badge?: number
  badgeTone?: "needs"
  /** What the count means, for a reader who cannot see its colour. */
  badgeLabel?: string
  /** Something runs: the icon breathes (`source-list.css` › Rows). */
  running?: boolean
  /** What hangs beneath the row: a channel's pinned sessions. */
  submenu?: ReactNode
  /** Controls at the row's end, shown under the pointer or keyboard focus. */
  actions?: ReactNode
  onClick: () => void
}) {
  return (
    <SidebarMenuItem
      size="xs"
      className={className}
      isActive={active}
      unread={unread}
      aria-current={current ? "page" : undefined}
      // The kit lays the count beside the control, not in it, and the icon is
      // hidden from readers: the name says both once.
      aria-label={[
        label,
        badge ? (badgeLabel ?? String(badge)) : null,
        running ? statusLabels.running : null,
      ]
        .filter(Boolean)
        .join(" ")}
      {...trailingRoom(badge ?? 0)}
      data-row-actions={actions ? "" : undefined}
      {...tooltip(title ?? "", { shortcut })}
      onClick={onClick}
      icon={<SidebarRowIcon name={icon} running={running} />}
      badge={
        badge ? (
          <Badge
            variant="secondary"
            className="workspace-badge"
            data-tone={badgeTone}
            aria-hidden
          >
            {badge}
          </Badge>
        ) : undefined
      }
      trailing={
        actions ? <span className="workspace-row-actions">{actions}</span> : undefined
      }
      // A coarse pointer has nothing to reveal on, so the kit keeps them shown there.
      showTrailingOnHover={Boolean(actions)}
      submenu={submenu}
    >
      {label}
    </SidebarMenuItem>
  )
}

/** A sidebar row's glyph, in its 16px column; it breathes while something runs. */
export function SidebarRowIcon({
  name,
  running = false,
}: {
  name: Parameters<typeof DesktopIcon>[0]["name"]
  running?: boolean
}) {
  return (
    <span className="workspace-row-icon" data-running={running || undefined}>
      <DesktopIcon name={name} />
    </span>
  )
}

/**
 * The room a row's label leaves for a count at its end, which the kit lays
 * over the row, by its digits (`source-list.css` › Rows reads it).
 */
function trailingRoom(count: number): Record<string, string | undefined> {
  if (!count) return {}
  return {
    "data-trailing": "count",
    "data-digits": String(Math.min(String(count).length, 3)),
  }
}
