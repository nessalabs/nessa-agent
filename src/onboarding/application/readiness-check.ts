/**
 * Asking the readiness source, more than once, without the answers crossing.
 *
 * Setup asks when it opens, and asks again whenever someone says to — a gateway
 * that was still starting up, or an agent that has just been signed in
 * elsewhere, is exactly the kind of thing a second ask answers differently. So
 * each ask carries the number it was, and only the newest one is delivered: a
 * stale "unreachable" written over a fresh "ready" would tell somebody their
 * gateway is down while it is answering.
 *
 * One ask runs at a time. Every ask reaches the readiness probe, which does real
 * work on the machine — on macOS a blocking subprocess per agent — so a person
 * pressing "Check again" while nothing visibly happens must not stack up probes.
 * Asking while an ask is in flight is that ask: it was started after whatever
 * was fixed, so its answer is the fresh one a second ask would go and fetch.
 *
 * Which ask is current and whether one is outstanding are the same fact seen
 * twice, so one object owns both. They were split once — the generation here,
 * the busy flag in the caller — and the pair could disagree: abandoning an ask
 * invalidated its answer without releasing the caller's flag, so the next ask
 * was refused as a duplicate of one whose answer had already been thrown away,
 * and nobody got an answer at all. Keeping them together is what makes
 * `abandon` mean "this ask is over" rather than half of it.
 *
 * Nothing here polls; every ask has a caller behind it.
 */

import type { AgentReadinessAnswer, AgentReadinessSource } from "./ports"

/** A readiness source asked so that only its newest answer counts. */
export interface ReadinessCheck {
  /**
   * Ask again, unless an ask is already in flight — then this *is* that ask.
   * Resolves when it does, whether or not its answer was delivered.
   */
  check(): Promise<void>
  /**
   * Drop whatever is in flight: its answer is no longer wanted, and it no
   * longer stands in the way of a genuinely fresh ask.
   */
  abandon(): void
}

/**
 * Wrap a readiness source so answers arrive in the order they were asked for.
 *
 * `receive` is called with at most one answer per ask, and never with an answer
 * that a later ask has already superseded or that `abandon` has dropped.
 *
 * `busyChanged` reports whether an ask is outstanding, for a control that wants
 * to show it is working: true when one starts, false when that same ask settles
 * or is abandoned. An ask that has been superseded no longer speaks for the
 * outstanding one, so a slow older answer cannot report the newer ask finished.
 */
export function createReadinessCheck(
  source: AgentReadinessSource,
  receive: (answer: AgentReadinessAnswer) => void,
  busyChanged: (busy: boolean) => void = () => undefined,
): ReadinessCheck {
  // The ask whose answer is still wanted, and the one still running. They part
  // company when an ask is abandoned or superseded, which is the whole point:
  // `inFlight` is then 0 while the abandoned ask is still out there resolving.
  let latest = 0
  let inFlight = 0
  let running: Promise<void> | undefined

  /** End the outstanding ask, if that is still the ask this generation is. */
  function release(generation: number): void {
    if (generation === 0 || inFlight !== generation) return
    inFlight = 0
    running = undefined
    busyChanged(false)
  }

  return {
    check() {
      if (inFlight !== 0) return running ?? Promise.resolve()
      const generation = (latest += 1)
      inFlight = generation
      busyChanged(true)
      running = (async () => {
        try {
          const answer = await source.read()
          // Not the ask anyone is waiting on any more.
          if (generation === latest) receive(answer)
        } finally {
          release(generation)
        }
      })()
      return running
    },
    abandon() {
      latest += 1
      release(inFlight)
    },
  }
}
