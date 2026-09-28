import { useCallback, useEffect, useState } from "react"

/**
 * A preference remembered in this webview's storage and shared by every
 * reader in the window: choosing it in Settings changes the window at once,
 * and a window beside it follows through the storage event. Theme, icon
 * family, workspace layout and tint are each one of these.
 *
 * Storage can be missing or refuse access (a private window, cleared site
 * data). Reading then falls back to what `parse` makes of nothing; choosing
 * still applies to this window, and simply is not remembered.
 *
 * `parse` owns what a stored or announced value means, and is given whatever
 * arrived — a string, `null`, or anything else — so a value this build does
 * not know falls back rather than leaking through.
 */
export interface StoredPreference<T> {
  /** The value and a way to choose another; every reader in the window follows. */
  usePreference(): readonly [T, (next: T) => void]
}

export function storedPreference<T>({
  key,
  event,
  parse,
  serialize = String,
  storage = () => window.localStorage,
}: {
  /** The storage key. */
  key: string
  /** The window event that tells this window's readers of a change. */
  event: string
  parse: (stored: unknown) => T
  serialize?: (value: T) => string
  /** Where it is remembered: the webview's local storage, unless a test passes its own. */
  storage?: () => Pick<Storage, "getItem" | "setItem">
}): StoredPreference<T> {
  const read = (): T => {
    try {
      return parse(storage().getItem(key))
    } catch {
      return parse(null)
    }
  }

  function usePreference() {
    const [value, setValue] = useState<T>(read)

    useEffect(() => {
      const follow = (change: Event) => {
        if (change instanceof StorageEvent && change.key !== null && change.key !== key)
          return
        // This window's own change carries its value, so it applies even
        // where storage refused to keep it.
        setValue(change instanceof CustomEvent ? parse(change.detail) : read())
      }
      window.addEventListener(event, follow)
      window.addEventListener("storage", follow)
      return () => {
        window.removeEventListener(event, follow)
        window.removeEventListener("storage", follow)
      }
    }, [])

    const choose = useCallback((next: T) => {
      const stored = serialize(next)
      try {
        storage().setItem(key, stored)
      } catch {
        // Not remembered; the choice still applies to this window.
      }
      window.dispatchEvent(new CustomEvent(event, { detail: stored }))
    }, [])

    return [value, choose] as const
  }

  return { usePreference }
}
