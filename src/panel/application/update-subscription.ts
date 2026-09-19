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
  attach,
  ask,
  ready,
}: {
  /** Every listener, resolved once they are all really attached. */
  attach: (live: () => boolean) => Promise<Unlisten[]>
  /** The one question, asked only once `attach` has resolved. */
  ask: (live: () => boolean) => Promise<void>
  /** Runs when the host can be heard from, before the question. */
  ready?: () => void
}): Unlisten {
  let cancelled = false
  let attached: Unlisten[] = []
  const live = () => !cancelled

  void (async () => {
    const listeners = await attach(live)
    // Cancelled while attaching: these exist now and nobody wants them, so
    // they come down here. The cleanup below has already run and found nothing.
    if (cancelled) {
      for (const unlisten of listeners) unlisten()
      return
    }
    attached = listeners
    ready?.()
    await ask(live)
  })()

  return () => {
    cancelled = true
    for (const unlisten of attached) unlisten()
    attached = []
  }
}
