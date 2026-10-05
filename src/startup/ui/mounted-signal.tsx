import { useLayoutEffect } from "react"

/**
 * Tells the load fallback the first React commit finished.
 * Set before that commit, a throw during the initial render is ignored and
 * the loading screen stays up with no code.
 */
export function StartupMounted() {
  useLayoutEffect(() => {
    document.documentElement.dataset.nessaMounted = "1"
  }, [])
  return null
}
