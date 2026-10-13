/**
 * Asks already on their way, for one composed workspace.
 *
 * The overview can draw a question while the pane beneath it still holds
 * the same card. Both send through this record, which composition builds
 * once per store (`createDesktopDependencies`, `testStore`). A second card
 * joins the send already started and hears how it settled, so a refusal
 * lets every card try again. Two stores do not share a record.
 */

export interface QuestionFlights {
  /**
   * Runs `start` for `key`, or joins the run already on its way.
   * Every caller receives that run's outcome. Once it has settled, the
   * next call starts again.
   */
  join<Result>(key: string, start: () => Promise<Result>): Promise<Result>
}

/** A fresh record. Composition passes one into each store. */
export function createQuestionFlights(): QuestionFlights {
  const flights = new Map<string, Promise<unknown>>()
  return {
    join<Result>(key: string, start: () => Promise<Result>): Promise<Result> {
      const current = flights.get(key) as Promise<Result> | undefined
      if (current) return current
      const flight = start().finally(() => {
        if (flights.get(key) === flight) flights.delete(key)
      })
      flights.set(key, flight)
      return flight
    },
  }
}
