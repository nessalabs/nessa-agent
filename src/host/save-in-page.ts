/**
 * How long a saved file's object URL outlives the click that saves it: the
 * click starts the download from the URL, and the browser may still be
 * reading it after the click returns.
 */
export const SAVE_URL_LIFETIME_MS = 30_000

/**
 * Hands `bytes` to the browser to download as `name`, through an object URL
 * that is revoked once the browser has surely read it. A plain browser's own
 * way of saving a file (`pnpm dev`); a desktop host never takes this path —
 * its webview has no download handler, and its link policy refuses `blob:` —
 * and saves through its own dialog instead (`saveFile` in `window.ts`).
 */
export function saveInPage(bytes: Blob, name: string): "saved" {
  const url = URL.createObjectURL(bytes)
  const link = document.createElement("a")
  link.href = url
  link.download = name
  link.rel = "noopener"
  link.style.display = "none"
  document.body.append(link)
  try {
    link.click()
  } finally {
    link.remove()
    setTimeout(() => URL.revokeObjectURL(url), SAVE_URL_LIFETIME_MS)
  }
  return "saved"
}
