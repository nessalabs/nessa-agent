/**
 * One subscription to a target at a time, even across an open given up on
 * its way.
 *
 * The gateway allows one live subscription per target on a connection and
 * refuses a second (`subscription_duplicate`). A follower that stops before
 * its open is answered cannot close it yet: it has no handle. If it then
 * follows the same target again, its new subscribe can reach the gateway
 * while the first is still live, and is refused.
 *
 * So the rule lives here, once, for every follower: an open whose `signal`
 * is aborted before its answer is closed as soon as it is answered, and the
 * next open of the same target is sent only after that close has finished.
 * An open answered while its signal still holds goes ahead at once, and
 * aborting its signal later closes it. An open that is refused or answered
 * live lets the next one go at once: a second open of a live target is the
 * caller's duplicate, and the gateway says so.
 */
export type SubscriptionGate = <S extends { close(): Promise<void> }>(
  target: string,
  signal: AbortSignal | undefined,
  send: () => Promise<S>,
) => Promise<S>

export function createSubscriptionGate(): SubscriptionGate {
  // The last open of each target still in its way: the next waits for it.
  const ahead = new Map<string, Promise<void>>()
  return async (target, signal, send) => {
    const before = ahead.get(target)
    let release!: () => void
    const mine = new Promise<void>((done) => {
      release = done
    })
    const queued = before ? before.then(() => mine) : mine
    ahead.set(target, queued)
    void queued.then(() => {
      if (ahead.get(target) === queued) ahead.delete(target)
    })
    try {
      await before
      signal?.throwIfAborted()
      const opened = await send()
      if (signal?.aborted) {
        // Given up on its way: closed before the next open of this target.
        await opened.close().catch(() => undefined)
        signal.throwIfAborted()
      }
      const close = () => void opened.close().catch(() => undefined)
      signal?.addEventListener("abort", close, { once: true })
      return opened
    } finally {
      release()
    }
  }
}
