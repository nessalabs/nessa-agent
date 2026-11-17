import type { ReactNode } from "react"
import { Badge } from "@nessa-ui/react/badge"
import { SidebarMenuItem } from "@nessa-ui/react/sidebar"
import { DesktopIcon } from "../../../ui/icons"
import { tooltip } from "../../../ui/tooltip"
import { statusLabels } from "../../model/session-groups"
import { WorkingWords } from "../chrome/working-words"

/**
 * One row of the sidebar: the kit's `SidebarMenuItem`, dense, with an icon, a
 * label, and a count at its end. Something at work in it, its label
 * shimmers (`WorkingWords`). The row's actions show only under the pointer or
 * keyboard focus, in the count's cell: the kit swaps the count for them. The sidebar's skin (`source-list.css` › Rows)
 * gives it the window's inks.
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
  /** A count at the row's end; actions take its place under the pointer. */
  badge?: number
  badgeTone?: "needs"
  /** What the count means, for a reader who cannot see its colour. */
  badgeLabel?: string
  /** Something works in it: its label shimmers. */
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
      // The kit lays the count and the glyph beside the control, not in it:
      // its name says them once, and they stay silent beside it.
      aria-label={[
        label,
        badge ? (badgeLabel ?? String(badge)) : running ? statusLabels.running : null,
      ]
        .filter(Boolean)
        .join(" ")}
      {...trailingRoom(badge ? { count: badge } : null)}
      data-row-actions={actions ? "" : undefined}
      {...tooltip(title ?? "", { shortcut })}
      onClick={onClick}
      icon={<SidebarRowIcon name={icon} />}
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
      <WorkingWords working={Boolean(running)}>{label}</WorkingWords>
    </SidebarMenuItem>
  )
}

/** A sidebar row's glyph, in its 16px column. */
export function SidebarRowIcon({
  name,
}: {
  name: Parameters<typeof DesktopIcon>[0]["name"]
}) {
  return (
    <span className="workspace-row-icon">
      <DesktopIcon name={name} />
    </span>
  )
}

/**
 * The room a row's label leaves for the count at its end, which the kit
 * lays over the row, by its digits (`source-list.css` › Rows reads it).
 */
function trailingRoom(
  trailing: { count: number } | null,
): Record<string, string | undefined> {
  if (trailing === null) return {}
  return {
    "data-trailing": "count",
    "data-digits": String(Math.min(String(trailing.count).length, 3)),
  }
}
