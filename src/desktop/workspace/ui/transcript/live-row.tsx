import { useNow } from "../../adapters/dom/clock"
import type { Activity } from "../../model/transcript"
import { elapsed } from "../../model/time-labels"
import { StatusGlyph } from "../chrome/status-glyph"

/** What a running agent is doing now, and for how long, set where its reply will appear. */
export function LiveRow({ activity }: { activity: Activity }) {
  const now = useNow(1000)
  return (
    <div className="workspace-live" role="status">
      <StatusGlyph status="running" />
      <span className="workspace-shimmer">{activity.label}</span>
      <span className="workspace-live-time">{elapsed(activity.since, now)}</span>
    </div>
  )
}
