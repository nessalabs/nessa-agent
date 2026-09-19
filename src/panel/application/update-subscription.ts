/** Taking a listener down again. */
export type Unlisten = () => void

/**
 * Subscribe first, then ask — and be cancellable at every point in between.
 *
 * A subscription is a promise of a listener: until it resolves there is no
 * listener, and anything the host says in that window reaches nobody. Asking
 * for the current state before then does not close the gap, it only makes it
 * harder to see, because the answer can be "nothing yet" and the announcement
 * can land a moment later with nothing attached to hear it. The host announces
 * once per launch and is asked once, so what falls through that gap is gone
 * for the rest of the run.
 *
 * So the order is fixed here: every listener is attached, then the question is
 * asked. Anything announced before the question the host is still holding and
 * the answer carries; anything after reaches a listener. `ready` runs between
 * the two, for whatever must not be offered until the host can be heard from.
 *
 * `live` is handed to both sides rather than kept to itself, so a callback that
 * fires after cancellation and the work here agree on whether anyone is still
 * listening.
 */
export function attachThenAsk({
  listeners,
  ask,
  ready,
  failed,
}: {
  /**
   * One per listener, each resolving when *that* listener is attached.
   *
   * A list rather than one `Promise.all`, because the difference matters twice.
   * `Promise.all` hands back nothing when any of them rejects — including the
   * handles for the ones that succeeded, which are then attached with nobody
   * holding their `unlisten` and are never taken down. And a listener is live
   * the moment its own promise resolves, not when the last one does, so the
   * host can announce an update through one while another is still pending.
   */
  listeners: ((live: () => boolean) => Promise<Unlisten>)[]
  /** The one question, asked only once every listener is attached. */
  ask: (live: () => boolean) => Promise<void>
  /**
   * Runs when every listener is attached and before the question — so whatever
   * must not happen until the host can be *heard from* can wait for it.
   */
  ready?: () => void
  /**
   * Runs when attaching or asking threw. Nothing here can recover — the host is
   * unreachable for the life of this component — so what is owed is a
   * diagnostic rather than a retry.
   */
  failed?: (reason: unknown) => void
}): Unlisten {
  let cancelled = false
  const attached: Unlisten[] = []
  const live = () => !cancelled

  /** Take down everything held, once. */
  const release = () => {
    for (const unlisten of attached.splice(0)) unlisten()
  }

  void (async () => {
    // Attaching and asking fail differently, so they are caught differently.
    try {
      await Promise.all(
        listeners.map(async (attach) => {
          const unlisten = await attach(live)
          // Held the moment it exists. Cancelled in the meantime, it is taken
          // down here rather than left running with its handle discarded.
          if (cancelled) unlisten()
          else attached.push(unlisten)
        }),
      )
    } catch (reason) {
      // Nothing can be heard from the host, so the listeners that did attach
      // are no use: they would fire into a panel that was never told it was
      // listening, and the ones that did not attach never will.
      failed?.(reason)
      cancelled = true
      release()
      return
    }
    if (cancelled) return

    ready?.()

    try {
      await ask(live)
    } catch (reason) {
      // A different failure entirely, and the listeners must survive it. They
      // are attached and working; only the catch-up question was lost, so what
      // the host says from here on still arrives. Tearing them down here left a
      // panel that believed it was listening and was not — including an install
      // already started on the strength of `ready`, whose refusal then reached
      // nobody.
      failed?.(reason)
    }
  })()

  return () => {
    cancelled = true
    release()
  }
}
