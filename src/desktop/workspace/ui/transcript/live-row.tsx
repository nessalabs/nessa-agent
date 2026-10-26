import { useNow } from "../../../../clock"
import { shownActivity, type Activity, type Message } from "../../model/transcript"
import { elapsed } from "../../model/time-labels"
import { StatusGlyph } from "../chrome/status-glyph"

/**
 * What a running agent is doing now, and for how long, set where its reply
 * will appear (`after` is what is drawn above it). Where a live tool run
 * already shows it (`shownActivity`), it is said to a screen reader only:
 * the trace keeps no status of its own.
 */
export function LiveRow({
  activity,
  after,
}: {
  activity: Activity | null
  after: readonly Message[]
}) {
  if (!activity) return null
  return shownActivity(after, activity) ? (
    <DrawnRow activity={activity} />
  ) : (
    <p className="workspace-live-spoken" role="status">
      {activity.label}
    </p>
  )
}

function DrawnRow({ activity }: { activity: Activity }) {
  const now = useNow(1000)
  return (
    <div className="workspace-live" role="status">
      <StatusGlyph status="running" />
      <span className="workspace-shimmer">{activity.label}</span>
      <span className="workspace-live-time">{elapsed(activity.since, now)}</span>
    </div>
  )
}
