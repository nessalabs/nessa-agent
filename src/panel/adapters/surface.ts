import * as React from "react"

import { type Surface } from "../model"

export type { Surface }

const storageKey = "nessa.surface"

function read(): Surface {
  try {
    return window.localStorage.getItem(storageKey) === "clear" ? "clear" : "translucent"
  } catch {
    // Private windows and blocked site data throw rather than return null.
    return "translucent"
  }
}

/** The panel's surface, remembered across launches. */
export function useSurface(): [Surface, () => void] {
  const [surface, setSurface] = React.useState<Surface>(read)

  const toggle = React.useCallback(() => {
    setSurface((current) => {
      const next = current === "translucent" ? "clear" : "translucent"
      try {
        window.localStorage.setItem(storageKey, next)
      } catch {
        // Remembering the choice is a convenience, not a requirement.
      }
      return next
    })
  }, [])

  return [surface, toggle]
}
