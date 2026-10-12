import { HeldFileReadError, type HeldFileReader } from "../../application/ports"
import type { HeldFile, HeldFileFailure } from "../../model"

/**
 * Where one held file stands for whoever shows it. A failure that is
 * `unavailable` is being tried again (`HELD_FILE_RETRY_DELAYS_MS`); the
 * others are for good while the file is shown.
 */
export type HeldFileShown =
  | { readonly state: "loading" }
  | { readonly state: "shown"; readonly url: string; readonly bytes: Blob }
  | { readonly state: "failed"; readonly reason: HeldFileFailure }

/** How object URLs are made and let go of: the page's own in production. */
export type ObjectUrls = {
  create(bytes: Blob): string
  revoke(url: string): void
}

/** Waits `ms`, then calls `elapsed`; the returned function cancels the wait. */
export type HeldFileTimer = (ms: number, elapsed: () => void) => () => void

const pageUrls: ObjectUrls = {
  create: (bytes) => URL.createObjectURL(bytes),
  revoke: (url) => URL.revokeObjectURL(url),
}

const pageTimer: HeldFileTimer = (ms, elapsed) => {
  const id = setTimeout(elapsed, ms)
  return () => clearTimeout(id)
}

/**
 * How many held files one window reads at once. The gateway serves four reads
 * at a time across every surface and answers the rest busy; the panel and the
 * desktop window each take two of those four, and queue the rest, so a file
 * usually waits its turn rather than failing for the crowd. Not always: a
 * read abandoned mid-range may hold its slot until the gateway finishes that
 * range, and another device reads too. A read refused busy is asked again —
 * by the client for its range, then here after a wait — so it recovers.
 */
export const HELD_FILE_READS_AT_ONCE = 2

/**
 * How long a file that could not be read now (`unavailable`) waits before it
 * is asked for again, one wait per failure in a row, the last repeated for as
 * long as it is shown.
 */
export const HELD_FILE_RETRY_DELAYS_MS: readonly number[] = Object.freeze([
  1_000, 2_000, 5_000, 10_000, 30_000,
])

const loading: HeldFileShown = Object.freeze({ state: "loading" })

/** The reasons that hold for good: asking again, now, would answer the same. */
const settledFailures: ReadonlySet<HeldFileFailure> = new Set([
  "not-held",
  "deleted",
  "damaged",
])

const reasonOf = (error: unknown): HeldFileFailure =>
  error instanceof HeldFileReadError ? error.reason : "unavailable"

/**
 * The held files on screen in one window, each read back once and shown from
 * one object URL for as long as anything shows it.
 *
 * Keyed by the conversation and the file's whole name — digest, media type and
 * size — because that is what a conversation holds: the same bytes in another
 * conversation are read from that one, which may have let go of them.
 *
 * A file is read when the first thing starts showing it (`watch`), and its URL
 * revoked, or its read abandoned, when the last one stops. The release waits a
 * microtask, so something that stops and starts showing it in one commit — as
 * React's strict mode does with every effect — keeps what it had rather than
 * reading it again. A newer view of the conversation changes nothing here:
 * only what is on screen does.
 *
 * Every read, for showing or for saving, waits its turn in one queue,
 * {@link HELD_FILE_READS_AT_ONCE} at a time, so a transcript full of pictures
 * does not crowd the gateway into refusing them; one that is refused anyway
 * is asked for again.
 *
 * A file the conversation does not keep, a deleted conversation and bytes that
 * did not verify are answers that hold: each is kept while the file is shown,
 * so it is asked about once, not on every render, and showing it afresh asks
 * again. A file that could not be read now (`unavailable` — no gateway, a busy
 * one, a dropped connection) is asked for again after
 * {@link HELD_FILE_RETRY_DELAYS_MS}, for as long as it is shown, or at once
 * when somebody asks (`retry`).
 */
export function heldFileUrls(
  read: HeldFileReader,
  {
    urls = pageUrls,
    timer = pageTimer,
    readsAtOnce = HELD_FILE_READS_AT_ONCE,
  }: { urls?: ObjectUrls; timer?: HeldFileTimer; readsAtOnce?: number } = {},
) {
  type Entry = {
    shown: HeldFileShown
    holders: number
    listeners: Set<() => void>
    /** The read going now, or the last one; aborted when nothing shows it. */
    reading: AbortController
    /** Its bytes, while a read is going. */
    arriving: Promise<Blob> | undefined
    /** `unavailable` answers in a row, which pick the next wait. */
    failures: number
    /** Cancels the wait before asking again. */
    stopWaiting: (() => void) | undefined
  }
  const entries = new Map<string, Entry>()
  const keyOf = (conversationId: string, file: HeldFile) =>
    JSON.stringify([conversationId, file.digest, file.mimeType, file.size])

  // The queue: reads waiting their turn, and how many are going.
  const waiting: (() => void)[] = []
  let going = 0
  const startNext = () => {
    while (going < readsAtOnce && waiting.length > 0) waiting.shift()?.()
  }
  /** `read`, once its turn comes; given up, and out of the queue, if `signal` aborts first. */
  const queued = (conversationId: string, file: HeldFile, signal: AbortSignal) =>
    new Promise<Blob>((resolve, reject) => {
      const start = () => {
        signal.removeEventListener("abort", leave)
        going += 1
        read(conversationId, file, signal)
          .then(resolve, reject)
          .finally(() => {
            going -= 1
            startNext()
          })
      }
      const leave = () => {
        const at = waiting.indexOf(start)
        if (at >= 0) waiting.splice(at, 1)
        reject(new HeldFileReadError("unavailable", signal.reason))
      }
      if (signal.aborted) return leave()
      signal.addEventListener("abort", leave, { once: true })
      waiting.push(start)
      startNext()
    })

  const settle = (entry: Entry, shown: HeldFileShown) => {
    entry.shown = shown
    for (const listener of entry.listeners) listener()
  }
  const load = (key: string, entry: Entry, conversationId: string, file: HeldFile) => {
    entry.stopWaiting?.()
    entry.stopWaiting = undefined
    const reading = new AbortController()
    entry.reading = reading
    const arriving = queued(conversationId, file, reading.signal)
    entry.arriving = arriving
    arriving.then(
      (bytes) => {
        if (reading.signal.aborted) return
        entry.arriving = undefined
        entry.failures = 0
        settle(entry, { state: "shown", url: urls.create(bytes), bytes })
      },
      (error: unknown) => {
        if (reading.signal.aborted) return
        entry.arriving = undefined
        const reason = reasonOf(error)
        settle(entry, { state: "failed", reason })
        if (settledFailures.has(reason)) return
        const delays = HELD_FILE_RETRY_DELAYS_MS
        const delay = delays[Math.min(entry.failures, delays.length - 1)] ?? 0
        entry.failures += 1
        entry.stopWaiting = timer(delay, () => {
          entry.stopWaiting = undefined
          if (entries.get(key) === entry) load(key, entry, conversationId, file)
        })
      },
    )
  }
  /** Ask now for a file shown as not readable now, rather than waiting out the delay. */
  const askAgain = (
    key: string,
    entry: Entry,
    conversationId: string,
    file: HeldFile,
  ) => {
    if (entry.shown.state !== "failed" || settledFailures.has(entry.shown.reason)) return
    settle(entry, loading)
    load(key, entry, conversationId, file)
  }
  const release = (key: string, entry: Entry) => {
    if (entry.holders > 0 || entries.get(key) !== entry) return
    entries.delete(key)
    entry.reading.abort()
    entry.stopWaiting?.()
    if (entry.shown.state === "shown") urls.revoke(entry.shown.url)
  }
  return {
    /** What `file` is showing as now; `loading` before anything watched it. */
    current(conversationId: string, file: HeldFile): HeldFileShown {
      return entries.get(keyOf(conversationId, file))?.shown ?? loading
    },
    /**
     * Show `file`: read it if nothing shows it yet, and tell `listener` when
     * where it stands changes. The returned function stops showing it.
     */
    watch(conversationId: string, file: HeldFile, listener: () => void): () => void {
      const key = keyOf(conversationId, file)
      let entry = entries.get(key)
      if (!entry) {
        const created: Entry = {
          shown: loading,
          holders: 0,
          listeners: new Set(),
          reading: new AbortController(),
          arriving: undefined,
          failures: 0,
          stopWaiting: undefined,
        }
        entry = created
        entries.set(key, created)
        load(key, created, conversationId, file)
      }
      const held = entry
      held.holders += 1
      held.listeners.add(listener)
      let stopped = false
      return () => {
        if (stopped) return
        stopped = true
        held.holders -= 1
        held.listeners.delete(listener)
        void Promise.resolve().then(() => release(key, held))
      }
    },
    /**
     * Ask again now for a shown file that could not be read now
     * (`unavailable`), rather than waiting out the delay. Does nothing for one
     * that is shown, on its way, or failed for good.
     */
    retry(conversationId: string, file: HeldFile) {
      const key = keyOf(conversationId, file)
      const entry = entries.get(key)
      if (entry) askAgain(key, entry, conversationId, file)
    },
    /**
     * The file's verified bytes, for saving: the ones already shown when it is
     * on screen, the read already going when there is one, and otherwise read
     * now — in the same queue — and kept by nobody. A file on screen that
     * could not be read now is asked for again as its Retry would, so the
     * picture arrives with the save; one that failed for good refuses at once
     * for the same reason. Rejects as the reader does.
     */
    async bytes(conversationId: string, file: HeldFile, signal: AbortSignal) {
      const key = keyOf(conversationId, file)
      const entry = entries.get(key)
      if (entry?.shown.state === "shown") return entry.shown.bytes
      if (entry?.shown.state === "failed") {
        if (settledFailures.has(entry.shown.reason))
          throw new HeldFileReadError(entry.shown.reason)
        askAgain(key, entry, conversationId, file)
      }
      if (entry?.arriving) return entry.arriving
      return queued(conversationId, file, signal)
    },
  }
}

export type HeldFileUrls = ReturnType<typeof heldFileUrls>
