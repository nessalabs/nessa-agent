import { totals, type Experiment } from "../model/experiment"

/** How many changes are being graded now — or, when none are, why not. */
export function LiveStatus({ experiment }: { experiment: Experiment }) {
  const count = totals(experiment)
  if (count.live > 0)
    return (
      <span className="xp-live">
        <span className="xp-live-pulse" />
        {count.live} evaluating
      </span>
    )
  return (
    <span className="xp-live" data-idle>
      {count.spent >= experiment.budget ? "Budget spent" : "Between runs"}
    </span>
  )
}
