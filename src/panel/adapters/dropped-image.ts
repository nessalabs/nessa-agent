/** Website image drags usually carry HTML and a URL rather than file bytes. */
export function droppedImageUrl(data: Pick<DataTransfer, "getData">): string | null {
  const html = data.getData("text/html")
  const document = html ? new DOMParser().parseFromString(html, "text/html") : null
  // A selection can include an inline image or emoji. Its prose owns the drop;
  // only image-only HTML should take the download path ahead of pasted text.
  if (document?.body.textContent?.trim()) return null
  const source = document?.querySelector("img")?.getAttribute("src")
  const uri =
    source ||
    data
      .getData("text/uri-list")
      .split(/\r?\n/)
      .find((line) => line && !line.startsWith("#"))
  if (!uri) return null
  try {
    const url = new URL(uri)
    if (!["http:", "https:"].includes(url.protocol)) return null
    return source || /\.(png|jpe?g|gif|webp|avif|svg)(?:$|\?)/i.test(url.href)
      ? url.href
      : null
  } catch {
    return null
  }
}

/** Fetch only the explicitly dropped image, bounded by the local attachment limit and a 30-second deadline. */
export async function readDroppedImage(
  url: string,
  signal: AbortSignal,
  maxBytes: number,
): Promise<File> {
  const controller = new AbortController()
  const cancel = () => controller.abort(signal.reason)
  if (signal.aborted) cancel()
  else signal.addEventListener("abort", cancel, { once: true })
  const deadline = setTimeout(
    () => controller.abort(new DOMException("Image download timed out", "TimeoutError")),
    30_000,
  )
  try {
    const response = await fetch(url, {
      signal: controller.signal,
      credentials: "omit",
    })
    const type = response.headers.get("content-type")?.split(";")[0] ?? ""
    if (!response.ok || !type.startsWith("image/") || !response.body)
      throw new Error("Image unavailable")
    const reader = response.body.getReader()
    const chunks: Uint8Array<ArrayBuffer>[] = []
    let size = 0
    try {
      while (true) {
        const { done, value } = await reader.read()
        if (done) break
        size += value.byteLength
        if (size > maxBytes) throw new Error("Image too large")
        chunks.push(value)
      }
    } finally {
      await reader.cancel()
    }
    const name = new URL(url).pathname.split("/").pop() || "image"
    return new File(chunks, name, { type })
  } finally {
    clearTimeout(deadline)
    signal.removeEventListener("abort", cancel)
  }
}
