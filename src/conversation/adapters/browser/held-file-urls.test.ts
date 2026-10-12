import { describe, expect, it, vi } from "vitest"
import { HeldFileReadError, type HeldFileReader } from "../../application/ports"
import {
  HELD_FILE_READS_AT_ONCE,
  HELD_FILE_RETRY_DELAYS_MS,
  heldFileUrls,
  type HeldFileTimer,
  type ObjectUrls,
} from "./held-file-urls"

const image = { digest: `sha256:${"a".repeat(64)}`, mimeType: "image/png", size: 3 }
const other = { ...image, digest: `sha256:${"b".repeat(64)}` }
const nth = (index: number) => ({
  ...image,
  digest: `sha256:${index.toString(16).padStart(64, "0")}`,
})

/** Object URLs the test can count, made and let go of. */
function countedUrls() {
  const live = new Set<string>()
  let made = 0
  const urls: ObjectUrls = {
    create: () => {
      const url = `blob:test/${++made}`
      live.add(url)
      return url
    },
    revoke: (url) => {
      live.delete(url)
    },
  }
  return { urls, live }
}

/** A clock the test moves by hand: each wait is kept until `elapse` runs it. */
function manualTimer() {
  const waits: { ms: number; elapsed: () => void; cancelled: boolean }[] = []
  const timer: HeldFileTimer = (ms, elapsed) => {
    const wait = { ms, elapsed, cancelled: false }
    waits.push(wait)
    return () => {
      wait.cancelled = true
    }
  }
  const pending = () => waits.filter((wait) => !wait.cancelled)
  const elapse = () => {
    const due = pending()
    waits.length = 0
    for (const wait of due) wait.elapsed()
  }
  return { timer, pending, elapse }
}

/** A reader whose reads the test settles by hand. */
function manualReader() {
  const reads: {
    conversationId: string
    digest: string
    signal: AbortSignal
    resolve: (bytes: Blob) => void
    reject: (error: unknown) => void
  }[] = []
  const read: HeldFileReader = (conversationId, file, signal) =>
    new Promise((resolve, reject) =>
      reads.push({ conversationId, digest: file.digest, signal, resolve, reject }),
    )
  return { read, reads }
}

/** Lets the microtasks a release and a settled read wait for run. */
const released = () => new Promise<void>((resolve) => setTimeout(resolve, 0))

describe("heldFileUrls", () => {
  it("reads a file once for everything that shows it, and shows it from one URL", async () => {
    const { read, reads } = manualReader()
    const { urls, live } = countedUrls()
    const files = heldFileUrls(read, { urls })
    const toldA = vi.fn()
    const toldB = vi.fn()
    const stopA = files.watch("c", image, toldA)
    const stopB = files.watch("c", image, toldB)
    expect(files.current("c", image)).toEqual({ state: "loading" })
    expect(reads).toHaveLength(1)
    reads[0]!.resolve(new Blob(["abc"]))
    await vi.waitFor(() => expect(files.current("c", image).state).toBe("shown"))
    expect(toldA).toHaveBeenCalledOnce()
    expect(toldB).toHaveBeenCalledOnce()
    expect(live.size).toBe(1)
    // The same snapshot each time it is asked, until it changes.
    expect(files.current("c", image)).toBe(files.current("c", image))

    // Let go of only when the last stops showing it.
    stopA()
    await released()
    expect(live.size).toBe(1)
    stopB()
    await released()
    expect(live.size).toBe(0)
    expect(files.current("c", image)).toEqual({ state: "loading" })
  })

  it("reads the same bytes again from another conversation, which may not hold them", () => {
    const { read, reads } = manualReader()
    const files = heldFileUrls(read, { urls: countedUrls().urls, readsAtOnce: 3 })
    files.watch("one", image, () => {})
    files.watch("two", image, () => {})
    files.watch("one", other, () => {})
    expect(reads.map((item) => [item.conversationId, item.digest])).toEqual([
      ["one", image.digest],
      ["two", image.digest],
      ["one", other.digest],
    ])
  })

  it("keeps what it had when shown again in the same commit, as strict mode does", async () => {
    const { read, reads } = manualReader()
    const { urls, live } = countedUrls()
    const files = heldFileUrls(read, { urls })
    files.watch("c", image, () => {})()
    files.watch("c", image, () => {})
    reads[0]!.resolve(new Blob(["abc"]))
    await vi.waitFor(() => expect(files.current("c", image).state).toBe("shown"))
    await released()
    expect(reads).toHaveLength(1)
    expect(live.size).toBe(1)
  })

  it("abandons a read nobody shows any more, and makes no URL for it", async () => {
    const { read, reads } = manualReader()
    const { urls, live } = countedUrls()
    const files = heldFileUrls(read, { urls })
    files.watch("c", image, () => {})()
    await released()
    expect(reads[0]!.signal.aborted).toBe(true)
    reads[0]!.resolve(new Blob(["abc"]))
    await released()
    expect(live.size).toBe(0)
  })

  it.each(["not-held", "deleted", "damaged"] as const)(
    "keeps %s while it is shown, and asks again only when shown afresh",
    async (reason) => {
      const { read, reads } = manualReader()
      const clock = manualTimer()
      const files = heldFileUrls(read, { urls: countedUrls().urls, timer: clock.timer })
      const stop = files.watch("c", image, () => {})
      reads[0]!.reject(new HeldFileReadError(reason))
      await vi.waitFor(() =>
        expect(files.current("c", image)).toEqual({ state: "failed", reason }),
      )
      // Nothing waits to ask again, and asking now does nothing either.
      expect(clock.pending()).toHaveLength(0)
      files.retry("c", image)
      files.watch("c", image, () => {})()
      expect(reads).toHaveLength(1)
      stop()
      await released()
      files.watch("c", image, () => {})
      expect(reads).toHaveLength(2)
    },
  )

  it("asks again after a wait for a file it could not read now, longer each time, until it is shown", async () => {
    const { read, reads } = manualReader()
    const clock = manualTimer()
    const files = heldFileUrls(read, { urls: countedUrls().urls, timer: clock.timer })
    files.watch("c", image, () => {})
    for (const [attempt, delay] of HELD_FILE_RETRY_DELAYS_MS.slice(0, 3).entries()) {
      // Anything that is not a reader's refusal is `unavailable` too.
      reads[attempt]!.reject(attempt === 0 ? new Error("socket closed") : undefined)
      await vi.waitFor(() =>
        expect(clock.pending().map((wait) => wait.ms)).toEqual([delay]),
      )
      expect(files.current("c", image)).toEqual({
        state: "failed",
        reason: "unavailable",
      })
      clock.elapse()
      expect(reads).toHaveLength(attempt + 2)
    }
    reads[3]!.resolve(new Blob(["abc"]))
    await vi.waitFor(() => expect(files.current("c", image).state).toBe("shown"))
  })

  it("asks again at once when somebody retries, and stops waiting when nothing shows it", async () => {
    const { read, reads } = manualReader()
    const clock = manualTimer()
    const files = heldFileUrls(read, { urls: countedUrls().urls, timer: clock.timer })
    const stop = files.watch("c", image, () => {})
    reads[0]!.reject(new HeldFileReadError("unavailable"))
    await vi.waitFor(() => expect(files.current("c", image).state).toBe("failed"))
    files.retry("c", image)
    expect(files.current("c", image)).toEqual({ state: "loading" })
    expect(reads).toHaveLength(2)
    // The wait it was in is over: retrying does not ask twice.
    expect(clock.pending()).toHaveLength(0)
    reads[1]!.reject(new HeldFileReadError("unavailable"))
    await vi.waitFor(() => expect(clock.pending()).toHaveLength(1))
    stop()
    await released()
    expect(clock.pending()).toHaveLength(0)
  })

  it("reads a few at a time, so every file ends shown against a gateway that refuses a crowd", async () => {
    // The gateway's own bound: past it, a read is refused busy.
    const gatewayReadsAtOnce = 4
    let going = 0
    let most = 0
    const read: HeldFileReader = async (_conversation, file) => {
      if (going >= gatewayReadsAtOnce) throw new HeldFileReadError("unavailable")
      going += 1
      most = Math.max(most, going)
      await new Promise((resolve) => setTimeout(resolve, 1))
      going -= 1
      return new Blob([file.digest])
    }
    const clock = manualTimer()
    const files = heldFileUrls(read, { urls: countedUrls().urls, timer: clock.timer })
    const many = Array.from({ length: 24 }, (_, index) => nth(index))
    for (const file of many) files.watch("c", file, () => {})
    await vi.waitFor(() =>
      expect(many.map((file) => files.current("c", file).state)).toEqual(
        many.map(() => "shown"),
      ),
    )
    expect(most).toBe(HELD_FILE_READS_AT_ONCE)
    // Not one waited out a refusal.
    expect(clock.pending()).toHaveLength(0)
  })

  it("takes a file nobody shows any more out of the queue before it is read", async () => {
    const { read, reads } = manualReader()
    const files = heldFileUrls(read, { urls: countedUrls().urls, readsAtOnce: 1 })
    files.watch("c", image, () => {})
    files.watch("c", other, () => {})()
    await released()
    reads[0]!.resolve(new Blob(["abc"]))
    await released()
    expect(reads.map((item) => item.digest)).toEqual([image.digest])
  })

  it("saves from the bytes on screen, joins a read already going, and reads afresh what is not shown", async () => {
    const { read, reads } = manualReader()
    const files = heldFileUrls(read, { urls: countedUrls().urls })
    files.watch("c", image, () => {})
    // Asked while the file is still on its way: the same read, not a second.
    const joined = files.bytes("c", image, new AbortController().signal)
    expect(reads).toHaveLength(1)
    const shown = new Blob(["abc"])
    reads[0]!.resolve(shown)
    expect(await joined).toBe(shown)
    await vi.waitFor(() => expect(files.current("c", image).state).toBe("shown"))
    expect(await files.bytes("c", image, new AbortController().signal)).toBe(shown)
    expect(reads).toHaveLength(1)
    const saving = files.bytes("c", other, new AbortController().signal)
    expect(reads).toHaveLength(2)
    const fresh = new Blob(["xyz"])
    reads[1]!.resolve(fresh)
    expect(await saving).toBe(fresh)
  })

  it("saves a shown file it could not read now through the same read, so the picture comes too", async () => {
    const { read, reads } = manualReader()
    const clock = manualTimer()
    const files = heldFileUrls(read, { urls: countedUrls().urls, timer: clock.timer })
    files.watch("c", image, () => {})
    reads[0]!.reject(new HeldFileReadError("unavailable"))
    await vi.waitFor(() => expect(clock.pending()).toHaveLength(1))
    const saving = files.bytes("c", image, new AbortController().signal)
    expect(files.current("c", image)).toEqual({ state: "loading" })
    expect(reads).toHaveLength(2)
    expect(clock.pending()).toHaveLength(0)
    const bytes = new Blob(["abc"])
    reads[1]!.resolve(bytes)
    expect(await saving).toBe(bytes)
    await vi.waitFor(() => expect(files.current("c", image).state).toBe("shown"))
  })

  it("refuses to save a shown file that failed for good, without asking again", async () => {
    const { read, reads } = manualReader()
    const files = heldFileUrls(read, { urls: countedUrls().urls })
    files.watch("c", image, () => {})
    reads[0]!.reject(new HeldFileReadError("not-held"))
    await vi.waitFor(() => expect(files.current("c", image).state).toBe("failed"))
    await expect(
      files.bytes("c", image, new AbortController().signal),
    ).rejects.toMatchObject({ reason: "not-held" })
    expect(reads).toHaveLength(1)
  })

  it("stops a save waiting on a shared read once the save is abandoned, and the read goes on", async () => {
    const { read, reads } = manualReader()
    const files = heldFileUrls(read, { urls: countedUrls().urls })
    files.watch("c", image, () => {})
    const saving = new AbortController()
    const joined = files.bytes("c", image, saving.signal).catch((error: unknown) => error)
    saving.abort()
    expect(await joined).toMatchObject({ reason: "unavailable" })
    // The picture's own read was not abandoned with it.
    expect(reads[0]!.signal.aborted).toBe(false)
    reads[0]!.resolve(new Blob(["abc"]))
    await vi.waitFor(() => expect(files.current("c", image).state).toBe("shown"))
  })
})
