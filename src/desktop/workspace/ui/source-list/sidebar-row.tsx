import type { ReactNode } from "react"
import { Badge } from "@nessa-ui/react/badge"
import { SidebarMenuItem } from "@nessa-ui/react/sidebar"
import { DesktopIcon } from "../../../ui/icons"
import { tooltip } from "../../../ui/tooltip"
import { statusLabels } from "../../model/session-groups"
import { StatusGlyph } from "../chrome/status-glyph"

/**
 * One row of the sidebar: the kit's `SidebarMenuItem`, dense, with an icon, a
 * label, and a count or a running glyph at its end. The sidebar's skin
 * (`source-list.css` › Rows) gives it the window's inks.
 */
export function SidebarRow({
  icon,
  label,
  title,
  shortcut,
  className,
  active,
  unread,
  badge,
  badgeTone,
  badgeLabel,
  running,
  submenu,
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
  unread?: boolean
  /** A count, shown instead of the running glyph. */
  badge?: number
  badgeTone?: "needs"
  /** What the count means, for a reader who cannot see its colour. */
  badgeLabel?: string
  running?: boolean
  /** What hangs beneath the row: a channel's pinned sessions. */
  submenu?: ReactNode
  onClick: () => void
}) {
  return (
    <SidebarMenuItem
      size="xs"
      className={className}
      isActive={active}
      unread={unread}
      aria-current={active ? "page" : undefined}
      // The kit lays the count and the glyph beside the control, not in it:
      // its name says them, as the row's words did when they were inside.
      aria-label={[
        label,
        badge ? (badgeLabel ?? String(badge)) : running ? statusLabels.running : null,
      ]
        .filter(Boolean)
        .join(" ")}
      {...trailingRoom(badge ? { count: badge } : running ? "glyph" : null)}
      {...tooltip(title ?? "", { shortcut })}
      onClick={onClick}
      icon={<SidebarRowIcon name={icon} />}
      badge={
        badge ? (
          <Badge
            variant="secondary"
            className="workspace-badge"
            data-tone={badgeTone}
            aria-label={badgeLabel}
          >
            {badge}
          </Badge>
        ) : undefined
      }
      trailing={!badge && running ? <StatusGlyph status="running" /> : undefined}
      submenu={submenu}
    >
      {label}
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
 * The room a row's label leaves for what sits at its end, which the kit lays
 * over the row: a glyph's, or a count's by its digits (`source-list.css` ›
 * Rows reads it).
 */
function trailingRoom(
  trailing: "glyph" | { count: number } | null,
): Record<string, string | undefined> {
  if (trailing === null) return {}
  if (trailing === "glyph") return { "data-trailing": "glyph" }
  return {
    "data-trailing": "count",
    "data-digits": String(Math.min(String(trailing.count).length, 3)),
  }
}
