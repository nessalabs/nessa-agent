import type {
  ConversationPermission,
  ConversationReviewEntry,
} from "../generated/product.js"

/** One listed review, as far as it has been read. */
export type ReviewDetail =
  | { readonly state: "loading"; readonly entry: ConversationReviewEntry }
  | {
      readonly state: "failed"
      readonly entry: ConversationReviewEntry
      readonly error: unknown
    }
  | {
      readonly state: "loaded"
      readonly entry: ConversationReviewEntry
      readonly permission: ConversationPermission
    }

/** The clock a loader waits on between attempts; a test passes its own. */
export type ReviewDetailsTimer = {
  set(run: () => void, ms: number): unknown
  clear(handle: unknown): void
}

/** The first wait after a failed read; each later one doubles, up to the most. */
export const REVIEW_RETRY_FIRST_MS = 1000
/** The longest wait between two reads of one review. */
export const REVIEW_RETRY_MOST_MS = 30_000

const systemTimer: ReviewDetailsTimer = {
  set: (run, ms) => setTimeout(run, ms),
  clear: (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>),
}

/**
 * What each review a view lists shows, read from its live owner.
 *
 * A view lists a waiting review by its key alone. `follow` is told each view's
 * entries: a review newly listed is read with `read`; one no longer listed is
 * let go, and its read abandoned. A read that fails is tried again after a
 * wait that doubles from {@link REVIEW_RETRY_FIRST_MS} to
 * {@link REVIEW_RETRY_MOST_MS}, for as long as the review is listed; `retry`
 * tries at once. `changed` is called whenever a detail's state changes.
 * After `dispose` nothing runs: no read is asked, no wait is pending.
 *
 * A surface offers no answer on a review until its detail is `loaded`.
 */
export function reviewDetails({
  read,
  changed,
  timer = systemTimer,
}: {
  read: (review: string, signal: AbortSignal) => Promise<ConversationPermission>
  changed: () => void
  timer?: ReviewDetailsTimer
}) {
  type Held = {
    detail: ReviewDetail
    abort?: AbortController
    wait?: unknown
    delayMs: number
    waiters: ((detail: ReviewDetail | undefined) => void)[]
  }
  const held = new Map<string, Held>()
  let listed: readonly ConversationReviewEntry[] = []
  let disposed = false

  const tell = (one: Held, detail: ReviewDetail | undefined) => {
    for (const resolve of one.waiters.splice(0)) resolve(detail)
  }
  const settle = (held: Held) => {
    held.abort?.abort()
    held.abort = undefined
    if (held.wait !== undefined) timer.clear(held.wait)
    held.wait = undefined
  }
  const attempt = (key: string) => {
    const one = held.get(key)
    if (!one || disposed) return
    settle(one)
    const abort = (one.abort = new AbortController())
    read(key, abort.signal).then(
      (permission) => {
        if (one.abort !== abort || held.get(key) !== one) return
        one.abort = undefined
        one.delayMs = REVIEW_RETRY_FIRST_MS
        one.detail = { state: "loaded", entry: one.detail.entry, permission }
        tell(one, one.detail)
        changed()
      },
      (error: unknown) => {
        if (one.abort !== abort || held.get(key) !== one) return
        one.abort = undefined
        const wasFailed = one.detail.state === "failed"
        one.detail = { state: "failed", entry: one.detail.entry, error }
        tell(one, one.detail)
        const delay = one.delayMs
        one.delayMs = Math.min(delay * 2, REVIEW_RETRY_MOST_MS)
        one.wait = timer.set(() => {
          one.wait = undefined
          attempt(key)
        }, delay)
        if (!wasFailed) changed()
      },
    )
  }

  return {
    /** Read each review `entries` lists that is not held; let go of the rest. */
    follow(entries: readonly ConversationReviewEntry[]) {
      if (disposed) return
      listed = entries
      const keys = new Set(entries.map((entry) => entry.review))
      for (const [key, one] of held)
        if (!keys.has(key)) {
          settle(one)
          held.delete(key)
          tell(one, undefined)
        }
      for (const entry of entries) {
        if (held.has(entry.review)) continue
        held.set(entry.review, {
          detail: { state: "loading", entry },
          delayMs: REVIEW_RETRY_FIRST_MS,
          waiters: [],
        })
        attempt(entry.review)
      }
    },
    /** Each listed review's detail, in the order the last view listed them. */
    details(): readonly ReviewDetail[] {
      return listed.flatMap((entry) => {
        const one = held.get(entry.review)
        return one ? [one.detail] : []
      })
    },
    /** The detail of the review `key`, while it is listed. */
    get(key: string): ReviewDetail | undefined {
      return held.get(key)?.detail
    },
    /**
     * The detail of the review `key` once its read in progress settles,
     * loaded or failed; `undefined` once it is not listed.
     */
    settled(key: string): Promise<ReviewDetail | undefined> {
      const one = held.get(key)
      if (!one || one.detail.state !== "loading") return Promise.resolve(one?.detail)
      return new Promise((resolve) => one.waiters.push(resolve))
    },
    /** Read a listed review that failed again now, with the wait started over. */
    retry(key: string) {
      const one = held.get(key)
      if (!one || disposed || one.detail.state !== "failed") return
      one.delayMs = REVIEW_RETRY_FIRST_MS
      one.detail = { state: "loading", entry: one.detail.entry }
      changed()
      attempt(key)
    },
    /** Stop: every read is abandoned, every wait cleared. */
    dispose() {
      disposed = true
      for (const one of held.values()) {
        settle(one)
        tell(one, undefined)
      }
      held.clear()
      listed = []
    },
  }
}

/** A loader made by {@link reviewDetails}. */
export type ReviewDetails = ReturnType<typeof reviewDetails>
