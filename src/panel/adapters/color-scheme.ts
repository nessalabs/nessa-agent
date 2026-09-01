import * as React from "react"

const darkQuery = "(prefers-color-scheme: dark)"

function subscribe(callback: () => void) {
  const query = window.matchMedia(darkQuery)
  query.addEventListener("change", callback)
  return () => query.removeEventListener("change", callback)
}

/**
 * Follows the OS appearance and keeps Nessa's `dark` variant class on the
 * document in sync, which is how the design system's tokens switch.
 */
export function useColorScheme(): "light" | "dark" {
  const isDark = React.useSyncExternalStore(
    subscribe,
    () => window.matchMedia(darkQuery).matches,
    () => false,
  )

  React.useLayoutEffect(() => {
    document.documentElement.classList.toggle("dark", isDark)
  }, [isDark])

  return isDark ? "dark" : "light"
}
