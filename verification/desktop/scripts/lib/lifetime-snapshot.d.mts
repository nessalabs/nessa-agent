/** Types for `lifetime-snapshot.mjs`. The unit test imports that reader. */

export interface LifetimeSnapshotRead {
  readonly follows: readonly string[]
  readonly stops: number
  readonly closed: readonly string[]
}

/**
 * Run `read` where the fixture lives and return a copy of its counters.
 * `evaluate` is the page bridge (`page.evaluate`); the controlled test
 * calls `read` in process.
 */
export function readLifetimeCounters(
  evaluate: (read: () => LifetimeSnapshotRead | null) => Promise<unknown>,
): Promise<LifetimeSnapshotRead>
