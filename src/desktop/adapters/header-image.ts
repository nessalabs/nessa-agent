import { useCallback, useEffect, useState } from "react"
import {
  defaultHeaderFraming,
  parseHeaderFraming,
  type HeaderFraming,
} from "../model/header-image"

const database = "nessa.desktop"
const store = "header"
const key = "image"

/** Opens the webview's IndexedDB store for the header picture. */
function open(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open(database, 1)
    request.onupgradeneeded = () => request.result.createObjectStore(store)
    request.onsuccess = () => resolve(request.result)
    request.onerror = () => reject(request.error)
  })
}

/** Runs one request against the store and settles with its result. */
async function run<T>(
  mode: IDBTransactionMode,
  act: (objects: IDBObjectStore) => IDBRequest<T>,
): Promise<T> {
  const db = await open()
  try {
    return await new Promise<T>((resolve, reject) => {
      const request = act(db.transaction(store, mode).objectStore(store))
      request.onsuccess = () => resolve(request.result)
      request.onerror = () => reject(request.error)
    })
  } finally {
    db.close()
  }
}

/**
 * The header picture, remembered in this webview's IndexedDB, which holds
 * files as they are and has room for a large GIF. Storage can be missing or
 * refuse access; the header then shows the night scene, and a picture chosen
 * still shows in this window but is not remembered.
 */
export function useHeaderImage() {
  const [image, setImage] = useState<Blob | null>(null)

  useEffect(() => {
    let current = true
    run<unknown>("readonly", (objects) => objects.get(key))
      .then((stored) => {
        if (current && stored instanceof Blob) setImage(stored)
      })
      .catch(() => {})
    return () => {
      current = false
    }
  }, [])

  const choose = useCallback((next: Blob) => {
    setImage(next)
    run("readwrite", (objects) => objects.put(next, key)).catch(() => {})
  }, [])

  const clear = useCallback(() => {
    setImage(null)
    run("readwrite", (objects) => objects.delete(key)).catch(() => {})
  }, [])

  return [image, choose, clear] as const
}

const framingKey = "nessa.desktop.header-framing"

/**
 * How the header picture is framed, remembered in this webview's storage.
 * Storage can be missing or refuse access; the framing then starts centred
 * and still applies to this window.
 */
export function useHeaderFraming() {
  const [framing, setFraming] = useState<HeaderFraming>(() => {
    try {
      const stored = window.localStorage.getItem(framingKey)
      return parseHeaderFraming(stored === null ? null : JSON.parse(stored))
    } catch {
      return defaultHeaderFraming
    }
  })

  const keep = useCallback((next: HeaderFraming) => {
    setFraming(next)
    try {
      window.localStorage.setItem(framingKey, JSON.stringify(next))
    } catch {
      // Not remembered; the framing still applies to this window.
    }
  }, [])

  return [framing, keep] as const
}

const tintKey = "nessa.desktop.tint-from-picture"

/**
 * Whether the window takes its theme from the header picture, remembered in
 * this webview's storage; on unless the person turned it off.
 */
export function useTintFromPicture() {
  const [tint, setTint] = useState(() => {
    try {
      return window.localStorage.getItem(tintKey) !== "off"
    } catch {
      return true
    }
  })

  const choose = useCallback((next: boolean) => {
    setTint(next)
    try {
      window.localStorage.setItem(tintKey, next ? "on" : "off")
    } catch {
      // Not remembered; the choice still applies to this window.
    }
  }, [])

  return [tint, choose] as const
}
