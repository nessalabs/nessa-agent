/**
 * The keyboard's way to a row's context menu: Shift-F10, or the context-menu
 * key, where a pointer would right-click. The menus open on the page's
 * `contextmenu` event, so the key sends one to the focused row, at its
 * leading edge; not every webview does that for these keys itself.
 */
export function contextMenuFromKey(event: {
  key: string
  shiftKey: boolean
  target: EventTarget
  preventDefault(): void
}): boolean {
  if (event.key !== "ContextMenu" && !(event.shiftKey && event.key === "F10"))
    return false
  if (!(event.target instanceof HTMLElement)) return false
  event.preventDefault()
  const box = event.target.getBoundingClientRect()
  event.target.dispatchEvent(
    new MouseEvent("contextmenu", {
      bubbles: true,
      cancelable: true,
      clientX: box.left + 16,
      clientY: box.top + box.height / 2,
    }),
  )
  return true
}
