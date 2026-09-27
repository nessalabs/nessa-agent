import { afterEach, expect, it, vi } from "vitest"
const MAX_ATTACHMENT_BYTES = 20 * 1024 * 1024
import { droppedImageUrl, readDroppedImage } from "./dropped-image"

afterEach(() => {
  vi.unstubAllGlobals()
  vi.useRealTimers()
})

it("keeps text selections with inline images on the pasted-text path", () => {
  vi.stubGlobal(
    "DOMParser",
    class {
      parseFromString() {
        return {
          body: { textContent: "Selected prose with an emoji" },
          querySelector: () => ({ getAttribute: () => "https://example.com/emoji.png" }),
        }
      }
    },
  )
  expect(
    droppedImageUrl({
      getData: (type) =>
        type === "text/html"
          ? '<p>Selected prose with an emoji<img src="https://example.com/emoji.png"></p>'
          : type === "text/uri-list"
            ? "https://example.com/emoji.png"
            : "Selected prose with an emoji",
    }),
  ).toBeNull()
})

it("accepts image-only HTML with whitespace and a browser link wrapper", () => {
  vi.stubGlobal(
    "DOMParser",
    class {
      parseFromString() {
        return {
          body: { textContent: " \n " },
          querySelector: () => ({ getAttribute: () => "https://example.com/photo.png" }),
        }
      }
    },
  )
  expect(
    droppedImageUrl({
      getData: (type) =>
        type === "text/html"
          ? '<a href="https://example.com"> <img src="https://example.com/photo.png"> </a>'
          : "",
    }),
  ).toBe("https://example.com/photo.png")
})

it("recognizes image URLs but rejects executable URLs", () => {
  expect(
    droppedImageUrl({
      getData: (type) =>
        type === "text/uri-list" ? "https://example.com/photo.png" : "",
    }),
  ).toBe("https://example.com/photo.png")
  expect(
    droppedImageUrl({
      getData: (type) => (type === "text/uri-list" ? "javascript:photo.png" : ""),
    }),
  ).toBeNull()
})
it("reads the selected image without credentials", async () => {
  vi.useFakeTimers()
  const fetcher = vi
    .fn()
    .mockResolvedValue(
      new Response("bytes", { headers: { "content-type": "image/png" } }),
    )
  vi.stubGlobal("fetch", fetcher)
  const signal = new AbortController().signal
  const file = await readDroppedImage(
    "https://example.com/photo.png",
    signal,
    MAX_ATTACHMENT_BYTES,
  )
  expect(file.name).toBe("photo.png")
  expect(file.type).toBe("image/png")
  expect(file.size).toBe(5)
  expect(fetcher).toHaveBeenCalledWith("https://example.com/photo.png", {
    signal: expect.any(AbortSignal),
    credentials: "omit",
  })
  expect(vi.getTimerCount()).toBe(0)
})

it.each(["deadline", "caller"])(
  "aborts a stalled image response on %s cancellation",
  async (cause) => {
    vi.useFakeTimers()
    const caller = new AbortController()
    let downloadSignal: AbortSignal | undefined
    vi.stubGlobal(
      "fetch",
      vi.fn(async (_url: string, options: RequestInit) => {
        downloadSignal = options.signal as AbortSignal
        const stream = new ReadableStream({
          start(controller) {
            downloadSignal!.addEventListener(
              "abort",
              () => controller.error(downloadSignal!.reason),
              { once: true },
            )
          },
        })
        return new Response(stream, { headers: { "content-type": "image/png" } })
      }),
    )
    const result = readDroppedImage(
      "https://example.com/photo.png",
      caller.signal,
      MAX_ATTACHMENT_BYTES,
    )
    const rejected = expect(result).rejects.toMatchObject({
      name: cause === "deadline" ? "TimeoutError" : "AbortError",
    })
    if (cause === "deadline") await vi.advanceTimersByTimeAsync(30_000)
    else caller.abort()
    await rejected
    expect(downloadSignal?.aborted).toBe(true)
    expect(vi.getTimerCount()).toBe(0)
  },
)
it("rejects non-image responses", async () => {
  vi.stubGlobal(
    "fetch",
    vi
      .fn()
      .mockResolvedValue(
        new Response("<html>", { headers: { "content-type": "text/html" } }),
      ),
  )
  await expect(
    readDroppedImage(
      "https://example.com/photo.png",
      new AbortController().signal,
      MAX_ATTACHMENT_BYTES,
    ),
  ).rejects.toThrow()
})
it("stops reading an oversized image stream", async () => {
  const cancel = vi.fn()
  const stream = new ReadableStream({
    start(controller) {
      controller.enqueue(new Uint8Array(MAX_ATTACHMENT_BYTES + 1))
    },
    cancel,
  })
  vi.stubGlobal(
    "fetch",
    vi
      .fn()
      .mockResolvedValue(
        new Response(stream, { headers: { "content-type": "image/png" } }),
      ),
  )
  await expect(
    readDroppedImage(
      "https://example.com/photo.png",
      new AbortController().signal,
      MAX_ATTACHMENT_BYTES,
    ),
  ).rejects.toThrow()
  expect(cancel).toHaveBeenCalled()
})
