import type {
  ConversationPermission,
  ConversationQuestion,
  ConversationQuestionEntry,
  ConversationReviewEntry,
} from "../generated/product.js"

/** One listed review or question, as far as it has been read. */
export type InteractionDetail<Entry, Shown> =
  | { readonly state: "loading"; readonly entry: Entry }
  | {
      readonly state: "failed"
      readonly entry: Entry
      readonly error: unknown
    }
  | {
      readonly state: "loaded"
      readonly entry: Entry
      readonly shown: Shown
    }
/** One listed review, as far as it has been read. */
export type ReviewDetail = InteractionDetail<
  ConversationReviewEntry,
  ConversationPermission
>
/** One listed question, as far as it has been read. */
export type QuestionDetail = InteractionDetail<
  ConversationQuestionEntry,
  ConversationQuestion
>

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
 * What each review or question a view lists shows, read from its live owner.
 *
 * A view lists a waiting review or question by a small entry under its key
 * (`keyOf`). `follow` is told each view's entries: one newly listed is read
 * with `read`; one no longer listed is let go, and its read abandoned. A read
 * that fails is tried again after a wait that doubles from
 * {@link REVIEW_RETRY_FIRST_MS} to {@link REVIEW_RETRY_MOST_MS}, for as long
 * as it is listed; `retry` tries at once. `changed` is called whenever a
 * detail's state changes. After `dispose` nothing runs: no read is asked, no
 * wait is pending.
 *
 * A surface offers no answer to one until its detail is `loaded`.
 */
export function interactionDetails<Entry, Shown>({
  keyOf,
  read,
  changed,
  timer = systemTimer,
}: {
  keyOf: (entry: Entry) => string
  read: (key: string, signal: AbortSignal) => Promise<Shown>
  changed: () => void
  timer?: ReviewDetailsTimer
}) {
  type Detail = InteractionDetail<Entry, Shown>
  type Held = {
    detail: Detail
    abort?: AbortController
    wait?: unknown
    delayMs: number
    waiters: ((detail: Detail | undefined) => void)[]
  }
  const held = new Map<string, Held>()
  let listed: readonly Entry[] = []
  let disposed = false

  const tell = (one: Held, detail: Detail | undefined) => {
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
      (shown) => {
        if (one.abort !== abort || held.get(key) !== one) return
        one.abort = undefined
        one.delayMs = REVIEW_RETRY_FIRST_MS
        one.detail = { state: "loaded", entry: one.detail.entry, shown }
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
    /** Read each one `entries` lists that is not held; let go of the rest. */
    follow(entries: readonly Entry[]) {
      if (disposed) return
      listed = entries
      const keys = new Set(entries.map(keyOf))
      for (const [key, one] of held)
        if (!keys.has(key)) {
          settle(one)
          held.delete(key)
          tell(one, undefined)
        }
      for (const entry of entries) {
        const key = keyOf(entry)
        if (held.has(key)) continue
        held.set(key, {
          detail: { state: "loading", entry },
          delayMs: REVIEW_RETRY_FIRST_MS,
          waiters: [],
        })
        attempt(key)
      }
    },
    /** Each listed one's detail, in the order the last view listed them. */
    details(): readonly Detail[] {
      return listed.flatMap((entry) => {
        const one = held.get(keyOf(entry))
        return one ? [one.detail] : []
      })
    },
    /** The detail of the one listed under `key`, while it is listed. */
    get(key: string): Detail | undefined {
      return held.get(key)?.detail
    },
    /**
     * The detail of the one listed under `key` once its read in progress
     * settles, loaded or failed; `undefined` once it is not listed.
     */
    settled(key: string): Promise<Detail | undefined> {
      const one = held.get(key)
      if (!one || one.detail.state !== "loading") return Promise.resolve(one?.detail)
      return new Promise((resolve) => one.waiters.push(resolve))
    },
    /** Read a listed one that failed again now, with the wait started over. */
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

/** What each review a view lists shows: {@link interactionDetails} by review key. */
export function reviewDetails(options: {
  read: (review: string, signal: AbortSignal) => Promise<ConversationPermission>
  changed: () => void
  timer?: ReviewDetailsTimer
}) {
  return interactionDetails<ConversationReviewEntry, ConversationPermission>({
    keyOf: (entry) => entry.review,
    ...options,
  })
}
/** All each question a view lists asks: {@link interactionDetails} by question key. */
export function questionDetails(options: {
  read: (question: string, signal: AbortSignal) => Promise<ConversationQuestion>
  changed: () => void
  timer?: ReviewDetailsTimer
}) {
  return interactionDetails<ConversationQuestionEntry, ConversationQuestion>({
    keyOf: (entry) => entry.question,
    ...options,
  })
}

/** A loader made by {@link reviewDetails}. */
export type ReviewDetails = ReturnType<typeof reviewDetails>
/** A loader made by {@link questionDetails}. */
export type QuestionDetails = ReturnType<typeof questionDetails>
