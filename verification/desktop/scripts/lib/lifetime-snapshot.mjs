/**
 * The local-tab fixture's live follow, stop and close counts.
 * `evaluate` runs in the page. The counts are read from the fixture owner
 * there (`window.__localTabLifetimes`), not from a rendered attribute.
 * Types: `lifetime-snapshot.d.mts`.
 *
 * @param {(read: () => import("./lifetime-snapshot.d.mts").LifetimeSnapshotRead | null) => Promise<unknown>} evaluate
 * @returns {Promise<import("./lifetime-snapshot.d.mts").LifetimeSnapshotRead>}
 */
export async function readLifetimeCounters(evaluate) {
  const read = await evaluate(() => {
    const owner = globalThis.__localTabLifetimes
    if (!owner || typeof owner.snapshot !== "function") return null
    return owner.snapshot()
  })
  if (
    !read ||
    !Array.isArray(read.follows) ||
    !Array.isArray(read.closed) ||
    typeof read.stops !== "number"
  ) {
    throw new Error("local tab lifetime counters have no live owner")
  }
  return {
    follows: [...read.follows],
    stops: read.stops,
    closed: [...read.closed],
  }
}
