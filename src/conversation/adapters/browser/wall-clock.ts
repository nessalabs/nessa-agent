/**
 * Unix milliseconds that move only forward and at a steady rate: one wall
 * sample, taken when this is made, plus monotonic time since. A permission's
 * countdown is put on this clock as its view arrives and read against it on
 * every tick, so an OS clock correction during a review neither expires the
 * card early nor stretches it; the gateway, not this clock, decides the review.
 */
export function monotonicWallClock(
  wall: () => number = () => Date.now(),
  monotonic: () => number = () => performance.now(),
): () => number {
  const origin = wall() - monotonic()
  return () => origin + monotonic()
}
