/**
 * The host context a view reads (`HostContext`), from the page: the theme the
 * document wears (its `dark` class, which the design system's tokens
 * follow), the browser's language, and the place's size as laid out,
 * followed as it changes. Nothing of the window's chrome lies over a place a
 * host draws in — the pane's header and the window's sit above its body —
 * so its safe area is none.
 *
 * The size is nessa_ui's `useMeasuredSize`: the place's content box in whole
 * CSS pixels, re-measured when its box resizes, its content changes, or a web
 * font loads — the last two being resizes WebKit does not always report.
 * `widgets.mjs --only host-size` holds what it reports to the place's box in
 * Chromium and WebKit, a card grown by its content or a font included.
 */
import { useMemo, type RefObject } from "react"
import { useMeasuredSize } from "@nessa-ui/react/lib/size-observer"
import type { HostContext } from "../../model/widget-state"

const noInsets = { top: 0, right: 0, bottom: 0, left: 0 } as const

export function useHostContext(place: RefObject<HTMLElement | null>): HostContext {
  const size = useMeasuredSize(place)
  const theme = document.documentElement.classList.contains("dark") ? "dark" : "light"
  const locale = navigator.language
  // One value while nothing in it changes, so a view memoised on it draws only for a change.
  return useMemo(
    () => ({ theme, locale, size, safeArea: noInsets }),
    [theme, locale, size],
  )
}
