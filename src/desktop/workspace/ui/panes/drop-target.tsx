import type { Zone } from "../../model/pane-layout"
import "./drop-target.css"

const labels: Record<"session" | "pane", Record<Zone, string>> = {
  session: {
    center: "Open Here",
    left: "Split Left",
    right: "Split Right",
    top: "Split Up",
    bottom: "Split Down",
  },
  pane: {
    center: "Swap Panes",
    left: "Move Left",
    right: "Move Right",
    top: "Move Above",
    bottom: "Move Below",
  },
}

/**
 * Where a drop would land on a pane: a half for a split, the middle to open
 * in place or swap. The fill keeps one size and changes zone by transform,
 * and the label travels to the middle of the zone it names, so the target
 * never lays itself out again. In a full workspace the middle says it
 * replaces what is there, before anything is let go.
 */
export function DropTarget({
  zone,
  kind,
  full,
}: {
  zone: Zone
  kind: "session" | "pane"
  /** No more panes fit: a session dropped in the middle takes this pane's place. */
  full: boolean
}) {
  return (
    <div className="workspace-drop" data-zone={zone} aria-hidden="true">
      <span>
        {zone === "center" && kind === "session" && full
          ? "Replace This Pane"
          : labels[kind][zone]}
      </span>
    </div>
  )
}
