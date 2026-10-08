/**
 * A new session's home in a pane changes shape with its pane
 * (`ui/panes/conversation.css`): the window's home in a large pane, a
 * conversation's shape in a small one. The stylesheet decides which, and
 * names it in `--workspace-home-settle`; this only tells when that name
 * changes under a home already on the page, and marks the home
 * `data-reshaped` so the change settles rather than jumps. A home that has
 * just appeared is not marked, so it plays nothing of its own.
 *
 * Marked as the resize is observed — after layout, before paint — so the
 * settling starts on the frame the shape changed.
 *
 * Returns a function that stops watching.
 */
export function settleOnReshape(home: HTMLElement): () => void {
  if (typeof ResizeObserver === "undefined") return () => {}
  const shape = () => getComputedStyle(home).getPropertyValue("--workspace-home-settle")
  // The first observation owns the initial shape. Reading it while the home
  // mounts forces layout before the browser's resize/paint delivery.
  let seen: string | undefined
  const observer = new ResizeObserver(() => {
    const next = shape()
    if (next === seen) return
    const hadShape = seen !== undefined
    seen = next
    if (hadShape) home.dataset.reshaped = ""
  })
  observer.observe(home)
  return () => observer.disconnect()
}
