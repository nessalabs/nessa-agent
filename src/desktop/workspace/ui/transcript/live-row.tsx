import { useNow } from "../../../../clock"
import { type Activity, type Message } from "../../model/transcript"
import { liveActivity, type TranscriptView } from "../../model/transcript-view"
import { elapsed } from "../../model/time-labels"
import { StatusGlyph } from "../chrome/status-glyph"

/**
 * What a running agent is doing now, and for how long, set where its reply
 * will appear (`after` is what is drawn above it). Where a live tool run
 * already shows it (`shownActivity`), it is said to a screen reader only:
 * the trace keeps no status of its own. It is one status either way, so a
 * new label is announced as it changes, drawn or not. The compact view draws
 * no trace, so there the row is always drawn and only says the agent is
 * working (`liveActivity`).
 */
export function LiveRow({
  activity: reported,
  after,
  view = "detailed",
}: {
  activity: Activity | null
  after: readonly Message[]
  /** How much of the agent's work to show; detailed where not said. */
  view?: TranscriptView
}) {
  if (!reported) return null
  const shown = liveActivity(view, after, reported)
  const drawn = shown !== null
  const activity = shown ?? reported
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
