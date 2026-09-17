import * as React from "react"

import { NOTHING_HELD, type HeldKeys } from "../model/shortcut-display"

/**
 * The keys held down right now, while `watching`.
 *
 * Modifiers are read from each event's own state rather than accumulated,
 * because a key can go up while the window is not looking — the desktop host
 * claims the completed accelerator, and a chord released after that would
 * otherwise stay lit forever. Blur clears everything for the same reason: keys
 * released in another window are not released here.
 */
export function useHeldKeys(watching: boolean): HeldKeys {
  const [held, setHeld] = React.useState<HeldKeys>(NOTHING_HELD)

  React.useEffect(() => {
    if (!watching) return
    function read(event: KeyboardEvent, down: boolean): HeldKeys {
      const key = event.key.toLowerCase()
      const modifier =
        key === "meta" || key === "control" || key === "alt" || key === "shift"
      return {
        meta: event.metaKey,
        ctrl: event.ctrlKey,
        alt: event.altKey,
        shift: event.shiftKey,
        key: down && !modifier ? key : undefined,
      }
    }
    const onDown = (event: KeyboardEvent) => setHeld(read(event, true))
    const onUp = (event: KeyboardEvent) => setHeld(read(event, false))
    const clear = () => setHeld(NOTHING_HELD)
    window.addEventListener("keydown", onDown)
    window.addEventListener("keyup", onUp)
    window.addEventListener("blur", clear)
    return () => {
      window.removeEventListener("keydown", onDown)
      window.removeEventListener("keyup", onUp)
      window.removeEventListener("blur", clear)
      setHeld(NOTHING_HELD)
    }
  }, [watching])

  return held
}
