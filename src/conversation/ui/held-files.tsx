import * as React from "react"
import type { HeldFileReader, HeldFileSaver } from "../application/ports"
import {
  heldFileUrls,
  type HeldFileShown,
  type HeldFileUrls,
} from "../adapters/browser/held-file-urls"
import type { HeldFile, HeldFileFailure } from "../model"

type HeldFiles = { urls: HeldFileUrls; save: HeldFileSaver }

const HeldFilesContext = React.createContext<HeldFiles | null>(null)

// A name for each reader the window is given, so a different one is a
// different provider (see `HeldFilesProvider`).
const readerKeys = new WeakMap<HeldFileReader, number>()
let readersSeen = 0
const keyOfReader = (read: HeldFileReader) => {
  let key = readerKeys.get(read)
  if (key === undefined) readerKeys.set(read, (key = ++readersSeen))
  return key
}

/**
 * Where the transcripts under it read held files back from — images a turn
 * named by reference, and files an agent published — and how they are saved.
 * One per window, from composition, so the same file on screen twice is read
 * once, and every read the window makes waits in one queue.
 *
 * A different reader is a different provider: what the old one read and
 * showed is let go of, and everything under it is read afresh through the new
 * one.
 *
 * Without one, nothing is read: a placeholder stays a placeholder, and a file
 * offers no way to save it.
 */
export function HeldFilesProvider({
  read,
  save,
  children,
}: {
  /** The window's one reader, from composition. */
  read: HeldFileReader
  /** The host's way of saving a file, from composition. */
  save: HeldFileSaver
  children: React.ReactNode
}) {
  return (
    <ReadingThrough key={keyOfReader(read)} read={read} save={save}>
      {children}
    </ReadingThrough>
  )
}

function ReadingThrough({
  read,
  save,
  children,
}: {
  read: HeldFileReader
  save: HeldFileSaver
  children: React.ReactNode
}) {
  const [urls] = React.useState(() => heldFileUrls(read))
  const value = React.useMemo(() => ({ urls, save }), [urls, save])
  return <HeldFilesContext.Provider value={value}>{children}</HeldFilesContext.Provider>
}

/**
 * Where `file`, held by `conversationId`, stands for this component: read back
 * on first sight, shown from one object URL while anything shows it, and let
 * go of after. `undefined` when there is nothing to read it with — no
 * provider, or no conversation yet.
 */
export function useHeldFile(
  conversationId: string | undefined,
  file: HeldFile,
): HeldFileShown | undefined {
  const urls = React.useContext(HeldFilesContext)?.urls
  const { digest, mimeType, size } = file
  const subscribe = React.useCallback(
    (changed: () => void) =>
      urls && conversationId
        ? urls.watch(conversationId, { digest, mimeType, size }, changed)
        : () => {},
    [urls, conversationId, digest, mimeType, size],
  )
  const snapshot = React.useCallback(
    () =>
      urls && conversationId
        ? urls.current(conversationId, { digest, mimeType, size })
        : undefined,
    [urls, conversationId, digest, mimeType, size],
  )
  return React.useSyncExternalStore(subscribe, snapshot, snapshot)
}

/**
 * Ask again, now, for a file shown as `unavailable` — it is asked again on
 * its own too, after a wait. `undefined` with nothing to read it with.
 */
export function useRetryHeldFile(
  conversationId: string | undefined,
  file: HeldFile,
): (() => void) | undefined {
  const urls = React.useContext(HeldFilesContext)?.urls
  const { digest, mimeType, size } = file
  const retry = React.useCallback(() => {
    if (urls && conversationId) urls.retry(conversationId, { digest, mimeType, size })
  }, [urls, conversationId, digest, mimeType, size])
  return urls && conversationId ? retry : undefined
}

/** Where saving one file stands: not asked, reading and handing it over, or not saved. */
export type HeldFileSaving = { state: "idle" } | { state: "saving" } | { state: "failed" }

/**
 * Save `file` under `name`: its verified bytes — the ones on screen when it is
 * shown, the read already going when there is one, read now otherwise —
 * handed to the host's own way of saving (`HeldFileSaver`). A save the person
 * cancels is no failure. `undefined` when there is nothing to read it with. A
 * read still going when the component goes is abandoned.
 */
export function useSaveHeldFile(
  conversationId: string | undefined,
  file: HeldFile,
  name: string,
): { saving: HeldFileSaving; save: (() => void) | undefined } {
  const held = React.useContext(HeldFilesContext)
  const [saving, setSaving] = React.useState<HeldFileSaving>({ state: "idle" })
  const going = React.useRef<AbortController | null>(null)
  React.useEffect(
    () => () => {
      going.current?.abort()
      // A strict-mode remount keeps this ref: nothing is going any more.
      going.current = null
    },
    [],
  )
  const { digest, mimeType, size } = file
  const save = React.useCallback(() => {
    if (!held || !conversationId || going.current) return
    const reading = new AbortController()
    going.current = reading
    setSaving({ state: "saving" })
    held.urls
      .bytes(conversationId, { digest, mimeType, size }, reading.signal)
      .then((bytes) => (reading.signal.aborted ? undefined : held.save(bytes, name)))
      .then(
        () => {
          if (reading.signal.aborted) return
          going.current = null
          setSaving({ state: "idle" })
        },
        () => {
          if (reading.signal.aborted) return
          going.current = null
          setSaving({ state: "failed" })
        },
      )
  }, [held, conversationId, digest, mimeType, size, name])
  return { saving, save: held && conversationId ? save : undefined }
}

/** What a person is told about a held file that could not be read back. */
export function heldFileFailureText(reason: HeldFileFailure): string {
  return {
    "not-held": "No longer kept",
    deleted: "Conversation deleted",
    damaged: "Did not arrive intact",
    unavailable: "Couldn’t load",
  }[reason]
}

/** What a person is told about a file that was not saved, whatever stopped it. */
export const HELD_FILE_NOT_SAVED_TEXT = "Couldn’t save"
