/**
 * The host context a view reads (`HostContext`), from the page: the theme the
 * document wears (its `dark` class, which the design system's tokens
 * follow), the browser's language, and the place's size as laid out,
 * followed as it changes. Nothing of the window's chrome lies over a place a
 * host draws in — the pane's header and the window's sit above its body —
 * so its safe area is none.
 *
 * The size is nessa_ui's `useMeasuredSize` (registry item `size-observer`),
 * which owns when an element is measured again: a box resize, a content
 * change, or a web font loading — the last two being resizes WebKit does not
 * always report. Its own `size-observer.test.ts` holds that each is reported.
 * `widgets.mjs --only host-size` holds what this reports to the place's box,
 * in whole pixels, in Chromium and WebKit.
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
