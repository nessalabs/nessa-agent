/**
 * Keeps the webview's own right-click menu to where it belongs
 * (`model/webview-menu.ts`): a right-click the window's own menus did not
 * take, anywhere but in editable or selected text, opens nothing.
 *
 * A menu of ours — a session's, a pane's — takes its right-click first and
 * prevents the webview's; this sees only what is left, on the window.
 */
import { useEffect } from "react"
import { webviewMenuOpens } from "../model/webview-menu"

/** The input types a person types text into; every other input is a control. */
const textInputTypes = new Set([
  "",
  "text",
  "search",
  "url",
  "email",
  "password",
  "tel",
  "number",
])

function inEditableText(target: Element): boolean {
  const field = target.closest("input, textarea, [contenteditable]")
  if (field instanceof HTMLTextAreaElement) return true
  if (field instanceof HTMLInputElement)
    return textInputTypes.has(field.getAttribute("type") ?? "")
  // `contenteditable="false"` inside editable text is the one way to say "not here".
  return field !== null && field.getAttribute("contenteditable") !== "false"
}

/** Whether the point is on the page's selected text, not merely near a selection. */
function onSelectedText(x: number, y: number): boolean {
  const selection = document.getSelection()
  if (!selection || selection.isCollapsed) return false
  for (let index = 0; index < selection.rangeCount; index += 1) {
    for (const rect of selection.getRangeAt(index).getClientRects()) {
      if (x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom)
        return true
    }
  }
  return false
}

/**
 * Mounted once, by the window. `inspectable` — a development build — keeps
 * the webview's menu, and its Inspect Element, on ⌥-right-click anywhere.
 */
export function useWebviewMenu({ inspectable }: { inspectable: boolean }): void {
  useEffect(() => {
    const onContextMenu = (event: MouseEvent) => {
      if (event.defaultPrevented || !(event.target instanceof Element)) return
      const opens = webviewMenuOpens(
        {
          inEditableText: inEditableText(event.target),
          onSelectedText: onSelectedText(event.clientX, event.clientY),
          withOption: event.altKey,
        },
        inspectable,
      )
      if (!opens) event.preventDefault()
    }
    window.addEventListener("contextmenu", onContextMenu)
    return () => window.removeEventListener("contextmenu", onContextMenu)
  }, [inspectable])
}
