import { useCallback, useEffect, useState } from "react"
import { parseIconFamily, type IconFamilyId } from "../model/icon-family"

const key = "nessa.desktop.icon-family"
/** Tells every reader in this window that the icon family changed. */
const changeEvent = "nessa:desktop-icon-family"

function read(): IconFamilyId {
  try {
    return parseIconFamily(window.localStorage.getItem(key))
  } catch {
    return parseIconFamily(null)
  }
}

/**
 * The chosen icon family, remembered in this webview's storage and shared by
 * every surface that reads it, so Settings and the window's chrome change
 * together. Storage can be missing or refuse access (private windows, cleared
 * site data); the window then starts on the default and simply does not
 * remember.
 */
export function useIconFamilyPreference() {
  const [family, setFamily] = useState<IconFamilyId>(read)

  useEffect(() => {
    const follow = (event: Event) => {
      setFamily(event instanceof CustomEvent ? parseIconFamily(event.detail) : read())
    }
    window.addEventListener(changeEvent, follow)
    window.addEventListener("storage", follow)
    return () => {
      window.removeEventListener(changeEvent, follow)
      window.removeEventListener("storage", follow)
    }
  }, [])

  const choose = useCallback((next: IconFamilyId) => {
    try {
      window.localStorage.setItem(key, next)
    } catch {
      // Not remembered; the choice still applies to this window.
    }
    window.dispatchEvent(new CustomEvent(changeEvent, { detail: next }))
  }, [])

  return [family, choose] as const
}
