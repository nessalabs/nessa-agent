// @vitest-environment jsdom
/**
 * Storage is read once, on the first reader, and answers later: whatever the
 * person does meanwhile is the newer word. A picture cleared while the read
 * is on its way stays cleared when the read finds the old one.
 */
import { act } from "react"
import { createRoot } from "react-dom/client"
import { expect, it, vi } from "vitest"

/** An IndexedDB whose one stored picture is handed over only when `answer` is called. */
function slowStorage(stored: Blob) {
  const reads: (() => void)[] = []
  const request = <T,>(result: () => T, now = true) => {
    const req = { result: undefined as T | undefined } as {
      result: T | undefined
      onsuccess?: () => void
      onerror?: () => void
    }
    const settle = () => {
      req.result = result()
      req.onsuccess?.()
    }
    if (now) queueMicrotask(settle)
    else reads.push(settle)
    return req
  }
  const db = {
    transaction: () => ({
      objectStore: () => ({
        get: () => request(() => stored, false),
        put: () => request(() => undefined),
        delete: () => request(() => undefined),
      }),
    }),
    close: () => {},
  }
  const indexedDB = {
    open: () => {
      const req = request(() => db) as { onupgradeneeded?: () => void }
      return req
    },
  }
  return { indexedDB, answer: () => reads.splice(0).forEach((settle) => settle()) }
}

it("keeps a picture cleared while storage was being read cleared", async () => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  const storage = slowStorage(new Blob(["png"], { type: "image/png" }))
  vi.stubGlobal("indexedDB", storage.indexedDB)
  URL.createObjectURL = () => "blob:stored"
  URL.revokeObjectURL = () => {}
  vi.resetModules()
  const { useHeaderImage } = await import("./header-image")
  let image: Blob | null = null
  let clear = () => {}
  function Reader() {
    const [shown, , clearing] = useHeaderImage()
    image = shown
    clear = clearing
    return null
  }
  const host = document.createElement("div")
  document.body.append(host)
  const root = createRoot(host)
  // The first reader asks storage; it has not answered yet.
  await act(async () => root.render(<Reader />))
  await act(async () => new Promise((resolve) => setTimeout(resolve, 0)))
  expect(image).toBeNull()
  // The person clears the picture, then storage answers with the old one.
  await act(async () => clear())
  await act(async () => {
    storage.answer()
    await new Promise((resolve) => setTimeout(resolve, 0))
  })
  expect(image).toBeNull()
  await act(async () => root.unmount())
  host.remove()
  vi.unstubAllGlobals()
})
