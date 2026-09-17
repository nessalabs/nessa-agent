/**
 * Asking the readiness source, more than once, without the answers crossing.
 *
 * Setup asks when it opens, and asks again whenever someone says to — a gateway
 * that was still starting up, or an agent that has just been signed in
 * elsewhere, is exactly the kind of thing a second ask answers differently. Two
 * asks in flight at once is therefore normal, and the older of them must not
 * land last: a stale "unreachable" written over a fresh "ready" would tell
 * somebody their gateway is down while it is answering.
 *
 * So each ask carries the number it was, and only the newest one is delivered.
 * Nothing here polls; every ask has a caller behind it.
 */

import type { AgentReadinessAnswer, AgentReadinessSource } from "./ports"

/** A readiness source asked so that only its newest answer counts. */
export interface ReadinessCheck {
  /** Ask again. Resolves when the ask does, whether or not it was delivered. */
  check(): Promise<void>
  /** Drop whatever is in flight: its answer is no longer wanted. */
  abandon(): void
}

/**
 * Wrap a readiness source so answers arrive in the order they were asked for.
 *
 * `receive` is called with at most one answer per ask, and never with an answer
 * that a later ask has already superseded or that `abandon` has dropped.
 */
export function createReadinessCheck(
  source: AgentReadinessSource,
  receive: (answer: AgentReadinessAnswer) => void,
): ReadinessCheck {
  let latest = 0
  return {
    async check() {
      const generation = (latest += 1)
      const answer = await source.read()
      // Not the ask anyone is waiting on any more.
      if (generation !== latest) return
      receive(answer)
    },
    abandon() {
      latest += 1
    },
  }
}
