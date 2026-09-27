import { useCallback, useState } from "react"
import { parseDesktopTheme, type DesktopThemeId } from "../model/theme"

const key = "nessa.desktop.theme"

/**
 * The chosen theme, remembered in this webview's storage. Storage can be
 * missing or refuse access (private windows, cleared site data); the window
 * then starts on the default and simply does not remember.
 */
export function useThemePreference() {
  const [theme, setTheme] = useState<DesktopThemeId>(() => {
    try {
      return parseDesktopTheme(window.localStorage.getItem(key))
    } catch {
      return parseDesktopTheme(null)
    }
  })

  const choose = useCallback((next: DesktopThemeId) => {
    setTheme(next)
    try {
      window.localStorage.setItem(key, next)
    } catch {
      // Not remembered; the choice still applies to this window.
    }
  }, [])

  return [theme, choose] as const
}
