import { useCallback, useSyncExternalStore } from "react"
import {
  defaultHeaderFraming,
  parseHeaderFraming,
  type HeaderFraming,
} from "../model/header-image"
import { storedPreference } from "./stored-preference"

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
 * The window's one header picture: read from storage once, and shared by the
 * home header and every conversation pane's sliver of it, so each shows the
 * same picture from one decoded file, and a new choice reaches all of them.
 */
interface Held {
  readonly blob: Blob
  /** The picture as it is, GIFs moving. */
  readonly url: string
  /** A GIF's first frame, still, once drawn; the picture itself for anything else. */
  still: string | null
}

let held: Held | null = null
let asked = false
const listeners = new Set<() => void>()
const notify = () => listeners.forEach((listener) => listener())

function hold(blob: Blob | null) {
  if (held) {
    URL.revokeObjectURL(held.url)
    if (held.still && held.still !== held.url) URL.revokeObjectURL(held.still)
  }
  held = blob ? { blob, url: URL.createObjectURL(blob), still: null } : null
  notify()
}

function subscribe(listener: () => void) {
  listeners.add(listener)
  if (!asked) {
    asked = true
    run<unknown>("readonly", (objects) => objects.get(key))
      .then((stored) => {
        // A picture chosen while storage was being read is the newer one.
        if (stored instanceof Blob && !held) hold(stored)
      })
      .catch(() => {})
  }
  return () => listeners.delete(listener)
}

/**
 * A GIF's first frame, drawn once to a still picture for the panes that are
 * not focused, so only the focused one moves. Any other picture is its own
 * still. A webview that cannot draw it shows no picture there, rather than a
 * moving one.
 */
function drawStill(picture: Held) {
  if (picture.blob.type !== "image/gif") {
    picture.still = picture.url
    notify()
    return
  }
  if (typeof createImageBitmap !== "function") return
  createImageBitmap(picture.blob)
    .then((frame) => {
      const canvas = document.createElement("canvas")
      canvas.width = frame.width
      canvas.height = frame.height
      canvas.getContext("2d")?.drawImage(frame, 0, 0)
      frame.close()
      canvas.toBlob((still) => {
        if (!still || held !== picture) return
        picture.still = URL.createObjectURL(still)
        notify()
      })
    })
    .catch(() => {})
}

/**
 * The header picture, remembered in this webview's IndexedDB, which holds
 * files as they are and has room for a large GIF. Storage can be missing or
 * refuse access; the header then shows the night scene, and a picture chosen
 * still shows in this window but is not remembered.
 */
export function useHeaderImage() {
  const image = useSyncExternalStore(subscribe, () => held?.blob ?? null)

  const choose = useCallback((next: Blob) => {
    hold(next)
    run("readwrite", (objects) => objects.put(next, key)).catch(() => {})
  }, [])

  const clear = useCallback(() => {
    hold(null)
    run("readwrite", (objects) => objects.delete(key)).catch(() => {})
  }, [])

  return [image, choose, clear] as const
}

/**
 * Where the header picture can be drawn from, or null for the night scene:
 * as it is, or — `still` — a GIF's first frame, which is null until drawn.
 */
export function useHeaderPictureUrl(still = false): string | null {
  return useSyncExternalStore(subscribe, () => {
    if (!held) return null
    if (!still) return held.url
    if (held.still === null) {
      const picture = held
      // Drawn once, after this read; the store tells its readers when it is ready.
      queueMicrotask(() => {
        if (held === picture && picture.still === null && !drawing.has(picture)) {
          drawing.add(picture)
          drawStill(picture)
        }
      })
    }
    return held.still
  })
}
const drawing = new WeakSet<Held>()

/**
 * How the header picture is framed, remembered in this webview's storage and
 * followed by every reader in the window — the home header and the panes'
 * slivers. Storage can be missing or refuse access; the framing then starts
 * centred and still applies to this window.
 */
const framingPreference = storedPreference({
  key: "nessa.desktop.header-framing",
  event: "nessa:header-framing",
  parse: (stored): HeaderFraming => {
    try {
      return parseHeaderFraming(typeof stored === "string" ? JSON.parse(stored) : null)
    } catch {
      return defaultHeaderFraming
    }
  },
  serialize: (framing: HeaderFraming) => JSON.stringify(framing),
})

export const useHeaderFraming = framingPreference.usePreference

/**
 * Whether the window takes its theme from the header picture, remembered in
 * this webview's storage; on unless the person turned it off. Settings and
 * the header menu both change it, and each follows the other.
 */
const tintPreference = storedPreference({
  key: "nessa.desktop.tint-from-picture",
  event: "nessa:tint-from-picture",
  parse: (stored) => stored !== "off",
  serialize: (tint: boolean) => (tint ? "on" : "off"),
})

export const useTintFromPicture = tintPreference.usePreference
