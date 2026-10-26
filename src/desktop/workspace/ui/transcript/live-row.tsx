import { useNow } from "../../../../clock"
import { shownActivity, type Activity, type Message } from "../../model/transcript"
import { elapsed } from "../../model/time-labels"
import { StatusGlyph } from "../chrome/status-glyph"

/**
 * What a running agent is doing now, and for how long, set where its reply
 * will appear (`after` is what is drawn above it). Where a live tool run
 * already shows it (`shownActivity`), it is said to a screen reader only:
 * the trace keeps no status of its own. It is one status either way, so a
 * new label is announced as it changes, drawn or not.
 */
export function LiveRow({
  activity,
  after,
}: {
  activity: Activity | null
  after: readonly Message[]
}) {
  if (!activity) return null
  const drawn = shownActivity(after, activity) !== null
  return (
    <div className={drawn ? "workspace-live" : "workspace-live-spoken"} role="status">
      {drawn ? <StatusGlyph status="running" /> : null}
      <span className={drawn ? "workspace-shimmer" : undefined}>{activity.label}</span>
      {drawn ? <LiveTime since={activity.since} /> : null}
    </div>
  )
}

/** How long the agent has been at it; the clock runs only while it is shown. */
function LiveTime({ since }: { since: number }) {
  const now = useNow(1000)
  return <span className="workspace-live-time">{elapsed(since, now)}</span>
}
