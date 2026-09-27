import { useCallback, useEffect, useState } from "react"
import { parseDesktopTheme, type DesktopThemeId } from "../model/theme"

const key = "nessa.desktop.theme"
/** Tells every reader in this window that the theme changed. */
const changeEvent = "nessa:desktop-theme"

function read(): DesktopThemeId {
  try {
    return parseDesktopTheme(window.localStorage.getItem(key))
  } catch {
    return parseDesktopTheme(null)
  }
}

/**
 * The chosen theme, remembered in this webview's storage and shared by every
 * surface that reads it, so Settings and the window change together. Storage
 * can be missing or refuse access (private windows, cleared site data); the
 * window then starts on the default and simply does not remember.
 */
export function useThemePreference() {
  const [theme, setTheme] = useState<DesktopThemeId>(read)

  useEffect(() => {
    const follow = (event: Event) => {
      setTheme(event instanceof CustomEvent ? parseDesktopTheme(event.detail) : read())
    }
    window.addEventListener(changeEvent, follow)
    window.addEventListener("storage", follow)
    return () => {
      window.removeEventListener(changeEvent, follow)
      window.removeEventListener("storage", follow)
    }
  }, [])

  const choose = useCallback((next: DesktopThemeId) => {
    try {
      window.localStorage.setItem(key, next)
    } catch {
      // Not remembered; the choice still applies to this window.
    }
    window.dispatchEvent(new CustomEvent(changeEvent, { detail: next }))
  }, [])

  return [theme, choose] as const
}
