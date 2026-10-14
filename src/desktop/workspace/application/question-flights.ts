/**
 * Asks already on their way, for one composed workspace.
 *
 * The overview can draw a question while the pane beneath it still holds
 * the same card. Both send through this record, which composition builds
 * once per store (`createDesktopDependencies`, `testStore`). A second card
 * joins the send already started when its choices and initiator are the
 * same, and hears how that send settled, so a refusal lets every card try
 * again. A different answer does not join that send and does not receive
 * its outcome. Two stores do not share a record. The states and the
 * orderings that can arrive are
 * `docs/design/desktop-question-flights.md`.
 */

/** Returned when a caller asks to join a send that is carrying a different answer. */
export const differentFlight = { status: "different" } as const

export interface QuestionFlights {
  /**
   * Runs `start` for `key` and `stamp`, or joins the run already on its way
   * when `stamp` is the same. A different stamp does not start and resolves
   * `differentFlight` instead of that run's outcome. Once the run has
   * settled, the next call starts again.
   */
  join<Result>(
    key: string,
    stamp: string,
    start: () => Promise<Result>,
  ): Promise<Result | typeof differentFlight>
}

/** A fresh record. Composition passes one into each store. */
export function createQuestionFlights(): QuestionFlights {
  const flights = new Map<
    string,
    { readonly stamp: string; readonly result: Promise<unknown> }
  >()
  return {
    join<Result>(key: string, stamp: string, start: () => Promise<Result>) {
      const current = flights.get(key)
      if (current) {
        return (
          current.stamp === stamp ? current.result : Promise.resolve(differentFlight)
        ) as Promise<Result | typeof differentFlight>
      }
      const result = start().finally(() => {
        if (flights.get(key)?.result === result) flights.delete(key)
      })
      flights.set(key, { stamp, result })
      return result
    },
  }
}
