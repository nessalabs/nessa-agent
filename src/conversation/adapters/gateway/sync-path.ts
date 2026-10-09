/**
 * Both desktop windows follow committed changes: a payloadless
 * `conversation.changed` ping, then a head check, then one view read when
 * the head moved.
 *
 * The catalogue watch is the list. One connection may watch one conversation
 * (`recordTargets` is 1), so this window keeps that watch on its main
 * connection and at most two more connections, one conversation each.
 * Anything past that, or a record watch that drops, is read when its
 * catalogue row changes. The catalogue watch dropping, or never registering,
 * is the poller's to resume. A full membership replace happens on the first
 * catch-up and when the stream's identity changes, including an access epoch
 * that moved.
 *
 * `recordsPage` is not folded here. The head is the position; the view is
 * still `conversation.read`, which owns the projection and the live state
 * the catalogue does not carry.
 */
import {
  NessaRpcError,
  bounds,
  type CatalogueDescriptor,
  type CatalogueEntryKey,
  type ConversationCatalogueManifestResult,
  type ConversationView,
  type RecordScope,
} from "@nessa/client"

/** Record watches one window holds: the main connection plus two extras, so two windows fit the principal cap of 8. */
export const MAX_RECORD_WATCHES = 3

const MAX_MANIFEST_PAGES = 64

const ACCESS_ENDED = new Set([
  "unauthorized",
  "forbidden",
  "not_bound",
  "wrong_owner",
  "wrong_receiver",
  "stale_epoch",
])

/** Why the follow stopped and the poller should resume. Revocation is separate and does not resume it. */
export type FollowFallback =
  "unbound" | "watch-refused" | "watch-ended" | "malformed" | "incomplete"

/** One catalogue row, parsed from the published payload. Running is not in that payload. */
export interface CatalogueRow {
  conversationId: string
  title: string | null
  preview: string | null
  updatedAtMs: number
  createdAtMs: number
  archived: boolean
}

/** What changed in the catalogue. `reset` replaces membership; otherwise apply `rows` and drop `removedIds`. */
export interface CatalogueUpdate {
  reset: boolean
  rows: CatalogueRow[]
  removedIds: string[]
}

export interface CommitSocket {
  conversation: {
    binding(): Promise<{ receiverId: string; accessEpoch: string }>
    read(conversationId: string): Promise<ConversationView>
  }
  records: {
    head(
      conversationId: string,
      receiverId: string,
      accessEpoch: string,
    ): Promise<{ scope: RecordScope; head: string }>
  }
  catalogue: {
    head(params: {
      receiverId: string
      accessEpoch: string
    }): Promise<{ scope: RecordScope; head: string }>
    manifest(params: {
      accessEpoch: string
      request: {
        maxEntries: number
        pass: {
          scope: RecordScope
          completed: string
          boundary: string
          cursor?: CatalogueEntryKey
          generation: string
        }
      }
    }): Promise<ConversationCatalogueManifestResult>
    resolve(params: {
      accessEpoch: string
      pass: {
        scope: RecordScope
        completed: string
        boundary: string
        cursor?: CatalogueEntryKey
        generation: string
      }
      descriptor: CatalogueDescriptor
      maxPayloadBytes: number
    }): Promise<{ entry: CatalogueDescriptor; payload: Uint8Array }>
  }
  watches: {
    records(params: {
      conversationId: string
      receiverId: string
      accessEpoch: string
    }): Promise<{ watchId: string }>
    catalogue(params: {
      receiverId: string
      accessEpoch: string
    }): Promise<{ watchId: string }>
    unwatch(watchId: string): Promise<unknown>
  }
  on(
    event: "conversation.changed" | "conversation.watchEnded",
    handler: (payload: { watchId: string; reason?: string }) => void,
  ): () => void
  onConnectionStateChange?(handler: (state: { status: string }) => void): () => void
  close(): void
}

/** Saved heads, so a new socket rechecks instead of walking the catalogue again. */
export interface CommitCheckpoint {
  receiverId: string
  accessEpoch: string
  catalogue?: { scope: RecordScope; completed: string; cursor?: CatalogueEntryKey }
  records: { conversationId: string; scope: RecordScope; head: string }[]
}

export interface CommitFollower {
  start(): Promise<"sync" | "fallback" | "revoked">
  stop(): void
  /** The set of conversations to watch changed. */
  retarget(): void
  /** Register watches again on this socket and recheck from the saved heads. */
  reregister(): Promise<void>
  checkpoint(): CommitCheckpoint | undefined
  /** Calls made after construction. `viewReads` counts `conversation.read`. */
  counts(): { viewReads: number; headReads: number; catalogueHeads: number }
}

export interface CommitFollowOptions {
  client: CommitSocket
  /** Another connection that can hold one record watch. Absent means only the main connection's single watch. */
  openRecordConnection?: () => Promise<CommitSocket | undefined>
  /** Conversations this window is showing, in the order they should be watched. */
  targets(): readonly string[]
  onView(conversationId: string, view: ConversationView): void
  onCatalogue(update: CatalogueUpdate): void
  onFallback(reason: FollowFallback): void
  onRevoked(): void
  /** Restored heads. Absent means the first catch-up replaces membership. */
  checkpoint?: CommitCheckpoint
}

interface RecordWatch {
  conversationId: string
  watchId: string
  socket: CommitSocket
  extra: boolean
  head?: string
  scope?: RecordScope
}

interface CataloguePlace {
  scope: RecordScope
  completed: string
  cursor?: CatalogueEntryKey
}

/** A client that can follow commits. A poller client has none of these. */
export function isCommitSocket(value: object): value is CommitSocket {
  const client = value as Partial<CommitSocket> & {
    conversation?: { binding?: unknown }
  }
  return (
    typeof client.conversation?.binding === "function" &&
    client.records !== undefined &&
    typeof client.records.head === "function" &&
    client.catalogue !== undefined &&
    client.watches !== undefined &&
    typeof client.on === "function"
  )
}

function rpcCode(error: unknown): string | undefined {
  return error instanceof NessaRpcError ? error.code : undefined
}

function sameScope(left: RecordScope, right: RecordScope): boolean {
  return (
    left.receiver === right.receiver &&
    left.origin === right.origin &&
    left.stream === right.stream &&
    left.incarnation === right.incarnation &&
    left.schema === right.schema &&
    left.accessEpoch === right.accessEpoch
  )
}

/** Canonical decimal compare. A longer string is greater; otherwise byte order. */
function decimalGreater(left: string, right: string): boolean {
  const a = left.replace(/^0+/, "") || "0"
  const b = right.replace(/^0+/, "") || "0"
  if (a.length !== b.length) return a.length > b.length
  return a > b
}

function headMoved(
  previous: string | undefined,
  next: string,
): "same" | "advanced" | "rewound" {
  if (previous === undefined) return "advanced"
  if (previous === next) return "same"
  return decimalGreater(next, previous) ? "advanced" : "rewound"
}

/** The published catalogue JSON, read only for the fields a list row needs. */
function rowFromPayload(id: string, payload: Uint8Array): CatalogueRow | undefined {
  let parsed: unknown
  try {
    parsed = JSON.parse(new TextDecoder().decode(payload))
  } catch {
    return undefined
  }
  if (typeof parsed !== "object" || parsed === null) return undefined
  const record = parsed as Record<string, unknown>
  if (!Object.hasOwn(record, "id") || record.id !== id) return undefined
  if (!Object.hasOwn(record, "createdAtMs") || typeof record.createdAtMs !== "number")
    return undefined
  const createdAtMs = record.createdAtMs
  if (!Object.hasOwn(record, "summary") || record.summary === null) {
    return {
      conversationId: id,
      title: null,
      preview: null,
      updatedAtMs: createdAtMs,
      createdAtMs,
      archived: false,
    }
  }
  const summary = record.summary
  if (typeof summary !== "object" || summary === null) return undefined
  const fields = summary as Record<string, unknown>
  if (!Object.hasOwn(fields, "updatedAtMs") || typeof fields.updatedAtMs !== "number")
    return undefined
  if (!Object.hasOwn(fields, "archived") || typeof fields.archived !== "boolean")
    return undefined
  const title = Object.hasOwn(fields, "title") ? fields.title : null
  const preview = Object.hasOwn(fields, "preview") ? fields.preview : null
  if (title !== null && typeof title !== "string") return undefined
  if (preview !== null && typeof preview !== "string") return undefined
  return {
    conversationId: id,
    title,
    preview,
    updatedAtMs: fields.updatedAtMs,
    createdAtMs,
    archived: fields.archived,
  }
}

/**
 * Follow one socket. `start` registers, then rechecks, so a ping that arrives
 * before the watch id is stored is recovered by that recheck. A ping during
 * the recheck sets a dirty bit and causes one more pull.
 */
export function createCommitFollower(options: CommitFollowOptions): CommitFollower {
  let generation = 0
  let stopped = false
  let admitted = false
  let outcome: "sync" | "fallback" | "revoked" | undefined
  let binding = options.checkpoint
    ? {
        receiverId: options.checkpoint.receiverId,
        accessEpoch: options.checkpoint.accessEpoch,
      }
    : undefined
  let catalogueWatchId: string | undefined
  let catalogue: CataloguePlace | undefined = options.checkpoint?.catalogue
  const savedRecords = new Map(
    (options.checkpoint?.records ?? []).map((record) => [record.conversationId, record]),
  )
  const recordWatches = new Map<string, RecordWatch>()
  const extras: CommitSocket[] = []
  const off: (() => void)[] = []
  const knownUpdated = new Map<string, number>()
  let viewReads = 0
  let headReads = 0
  let catalogueHeads = 0
  let cataloguePull: Promise<void> | undefined
  let catalogueDirty = false
  let catalogueForceReset = false
  /** Set while a catalogue pull is on the stack, so a nested request cannot wait for itself. */
  let catalogueDepth = 0
  const recordPulls = new Map<string, Promise<void>>()
  const recordDirty = new Set<string>()
  const recordDepth = new Map<string, number>()
  let handlingAccess = false
  let subscribed = false
  let resumeReady: () => void = () => {}
  const untilReady = new Promise<void>((resolve) => {
    resumeReady = resolve
  })
  let reregistering: Promise<void> = Promise.resolve()

  const finish = (
    next: "sync" | "fallback" | "revoked",
  ): "sync" | "fallback" | "revoked" => {
    outcome ??= next
    return outcome
  }

  const counts = () => ({ viewReads, headReads, catalogueHeads })

  const checkpoint = (): CommitCheckpoint | undefined => {
    if (!binding) return undefined
    return {
      receiverId: binding.receiverId,
      accessEpoch: binding.accessEpoch,
      ...(catalogue ? { catalogue } : {}),
      records: [...savedRecords.values()],
    }
  }

  function stop() {
    if (stopped) return
    stopped = true
    generation += 1
    for (const unsubscribe of off) unsubscribe()
    off.length = 0
    const watches = [...recordWatches.values()]
    recordWatches.clear()
    const catalogueId = catalogueWatchId
    catalogueWatchId = undefined
    void dropWatch(options.client, catalogueId)
    for (const watch of watches) void dropWatch(watch.socket, watch.watchId)
    for (const extra of extras) extra.close()
    extras.length = 0
  }

  async function dropWatch(socket: CommitSocket, watchId: string | undefined) {
    if (!watchId) return
    try {
      await socket.watches.unwatch(watchId)
    } catch {
      /* The connection may already be gone. */
    }
  }

  function subscribe() {
    if (subscribed) return
    subscribed = true
    off.push(
      options.client.on("conversation.changed", (payload) => {
        if (stopped) return
        if (payload.watchId === catalogueWatchId) {
          void pullCatalogue(false)
          return
        }
        const watch = [...recordWatches.values()].find(
          (item) => item.watchId === payload.watchId,
        )
        if (watch) void pullRecord(watch.conversationId)
      }),
      options.client.on("conversation.watchEnded", (payload) => {
        if (stopped) return
        if (payload.watchId === catalogueWatchId) {
          catalogueWatchId = undefined
          options.onFallback("watch-ended")
          finish("fallback")
          stop()
          return
        }
        endRecordWatch(payload.watchId)
      }),
    )
    for (const extra of extras) listenExtra(extra)
    if (options.client.onConnectionStateChange) {
      off.push(
        options.client.onConnectionStateChange((state) => {
          if (stopped || state.status !== "connected" || !admitted) return
          void reregister()
        }),
      )
    }
  }

  function listenExtra(socket: CommitSocket) {
    off.push(
      socket.on("conversation.changed", (payload) => {
        if (stopped) return
        const watch = [...recordWatches.values()].find(
          (item) => item.socket === socket && item.watchId === payload.watchId,
        )
        if (watch) void pullRecord(watch.conversationId)
      }),
      socket.on("conversation.watchEnded", (payload) => {
        if (stopped) return
        const watch = [...recordWatches.values()].find(
          (item) => item.socket === socket && item.watchId === payload.watchId,
        )
        if (watch) endRecordWatch(watch.watchId)
      }),
    )
  }

  function endRecordWatch(watchId: string) {
    for (const [id, watch] of recordWatches) {
      if (watch.watchId !== watchId) continue
      recordWatches.delete(id)
      if (!watch.extra) return
      const at = extras.indexOf(watch.socket)
      if (at !== -1) extras.splice(at, 1)
      watch.socket.close()
      return
    }
  }

  async function handleAccess(error: unknown): Promise<boolean> {
    const code = rpcCode(error)
    if (!code || !ACCESS_ENDED.has(code)) return false
    if (!admitted) {
      options.onFallback(code === "not_bound" ? "unbound" : "watch-refused")
      finish("fallback")
      stop()
      return true
    }
    // Another refusal is already rebinding. Stopping here would drop that
    // attempt and leave the window with neither watches nor the poller.
    if (handlingAccess) return true
    handlingAccess = true
    try {
      const next = await options.client.conversation.binding()
      if (stopped) return true
      const moved =
        !binding ||
        next.receiverId !== binding.receiverId ||
        next.accessEpoch !== binding.accessEpoch
      binding = next
      if (moved) {
        catalogue = undefined
        savedRecords.clear()
        const previous = catalogueWatchId
        catalogueWatchId = undefined
        await dropWatch(options.client, previous)
        for (const watch of [...recordWatches.values()]) {
          recordWatches.delete(watch.conversationId)
          await dropWatch(watch.socket, watch.watchId)
        }
        if (await registerCatalogue()) {
          await pullCatalogue(true)
          await retargetWatches()
        }
      }
      return true
    } catch (again) {
      const againCode = rpcCode(again)
      if (againCode && ACCESS_ENDED.has(againCode)) {
        options.onRevoked()
        finish("revoked")
        stop()
        return true
      }
      options.onFallback("watch-refused")
      finish("fallback")
      stop()
      return true
    } finally {
      handlingAccess = false
    }
  }

  async function readBinding(): Promise<boolean> {
    try {
      const next = await options.client.conversation.binding()
      const moved =
        binding !== undefined &&
        (next.receiverId !== binding.receiverId ||
          next.accessEpoch !== binding.accessEpoch)
      binding = next
      admitted = true
      if (moved) {
        catalogue = undefined
        savedRecords.clear()
      }
      return true
    } catch (error) {
      if (await handleAccess(error)) return false
      options.onFallback("watch-refused")
      finish("fallback")
      stop()
      return false
    }
  }

  async function registerCatalogue(): Promise<boolean> {
    if (!binding) return false
    try {
      const result = await options.client.watches.catalogue({
        receiverId: binding.receiverId,
        accessEpoch: binding.accessEpoch,
      })
      catalogueWatchId = result.watchId
      return true
    } catch (error) {
      if (await handleAccess(error)) return false
      options.onFallback("watch-refused")
      finish("fallback")
      stop()
      return false
    }
  }

  async function pullCatalogue(forceReset: boolean): Promise<void> {
    if (stopped) return
    if (catalogueDepth > 0) {
      catalogueDirty = true
      if (forceReset) catalogueForceReset = true
      return
    }
    if (cataloguePull) {
      catalogueDirty = true
      if (forceReset) catalogueForceReset = true
      return cataloguePull
    }
    const run = (async () => {
      catalogueDepth += 1
      const gen = generation
      try {
        do {
          catalogueDirty = false
          const reset = forceReset || catalogueForceReset || catalogue === undefined
          forceReset = false
          catalogueForceReset = false
          await pullCatalogueOnce(reset, gen)
        } while (catalogueDirty && generation === gen && !stopped)
      } finally {
        catalogueDepth -= 1
      }
    })()
    cataloguePull = run
    try {
      await run
    } finally {
      if (cataloguePull === run) cataloguePull = undefined
    }
  }

  async function pullCatalogueOnce(reset: boolean, gen: number): Promise<void> {
    if (!binding || stopped || gen !== generation) return
    let head: { scope: RecordScope; head: string }
    try {
      catalogueHeads += 1
      head = await options.client.catalogue.head({
        receiverId: binding.receiverId,
        accessEpoch: binding.accessEpoch,
      })
    } catch (error) {
      if (stopped || gen !== generation) return
      if (await handleAccess(error)) return
      options.onFallback("watch-refused")
      finish("fallback")
      stop()
      return
    }
    if (stopped || gen !== generation) return
    const identityChanged =
      catalogue !== undefined && !sameScope(catalogue.scope, head.scope)
    const replace = reset || identityChanged
    if (!replace && catalogue && catalogue.completed === head.head) return
    const pass = {
      scope: head.scope,
      completed: replace ? "0" : (catalogue?.completed ?? "0"),
      boundary: head.head,
      generation: "1",
      ...(replace || !catalogue?.cursor ? {} : { cursor: catalogue.cursor }),
    }
    const entries: CatalogueDescriptor[] = []
    let cursor = pass.cursor
    for (let page = 0; page < MAX_MANIFEST_PAGES; page++) {
      let manifest: ConversationCatalogueManifestResult
      try {
        manifest = await options.client.catalogue.manifest({
          accessEpoch: binding.accessEpoch,
          request: {
            maxEntries: bounds.maxCataloguePageEntries,
            pass: { ...pass, ...(cursor ? { cursor } : {}) },
          },
        })
      } catch (error) {
        if (stopped || gen !== generation) return
        if (await handleAccess(error)) return
        options.onFallback("watch-refused")
        finish("fallback")
        stop()
        return
      }
      if (stopped || gen !== generation) return
      entries.push(...manifest.entries)
      const last = manifest.entries[manifest.entries.length - 1]
      if (!manifest.hasMore) break
      if (!last) {
        options.onFallback("incomplete")
        finish("fallback")
        stop()
        return
      }
      cursor = last.key
      if (page === MAX_MANIFEST_PAGES - 1) {
        options.onFallback("incomplete")
        finish("fallback")
        stop()
        return
      }
    }
    const rows: CatalogueRow[] = []
    const removedIds: string[] = []
    const seen = new Set<string>()
    for (const entry of entries) {
      const id = entry.key.id
      seen.add(id)
      if (entry.deleted) {
        removedIds.push(id)
        knownUpdated.delete(id)
        continue
      }
      let resolved: { entry: CatalogueDescriptor; payload: Uint8Array }
      try {
        resolved = await options.client.catalogue.resolve({
          accessEpoch: binding.accessEpoch,
          pass,
          descriptor: entry,
          maxPayloadBytes: bounds.maxRecordResponseBytes,
        })
      } catch (error) {
        if (stopped || gen !== generation) return
        if (await handleAccess(error)) return
        options.onFallback("watch-refused")
        finish("fallback")
        stop()
        return
      }
      if (stopped || gen !== generation) return
      if (resolved.entry.deleted) {
        removedIds.push(id)
        knownUpdated.delete(id)
        continue
      }
      const row = rowFromPayload(id, resolved.payload)
      if (!row) {
        options.onFallback("malformed")
        finish("fallback")
        stop()
        return
      }
      rows.push(row)
    }
    if (replace) {
      for (const id of [...knownUpdated.keys()]) {
        if (seen.has(id)) continue
        removedIds.push(id)
        knownUpdated.delete(id)
      }
    }
    catalogue = { scope: head.scope, completed: head.head }
    for (const row of rows) knownUpdated.set(row.conversationId, row.updatedAtMs)
    if (stopped || gen !== generation) return
    options.onCatalogue({ reset: replace, rows, removedIds })
    await readUnwatched(rows, gen)
  }

  async function readUnwatched(rows: CatalogueRow[], gen: number): Promise<void> {
    const watched = new Set(recordWatches.keys())
    const wanted = new Set(options.targets())
    for (const row of rows) {
      if (!wanted.has(row.conversationId) || watched.has(row.conversationId)) continue
      await readView(row.conversationId, gen)
    }
  }

  async function readView(conversationId: string, gen: number): Promise<void> {
    if (stopped || gen !== generation) return
    try {
      viewReads += 1
      const view = await options.client.conversation.read(conversationId)
      if (stopped || gen !== generation) return
      if (view.conversationId === conversationId) options.onView(conversationId, view)
    } catch (error) {
      if (stopped || gen !== generation) return
      if (await handleAccess(error)) return
      /* A single conversation's read failing leaves the catalogue follow in place. */
    }
  }

  async function pullRecord(conversationId: string): Promise<void> {
    if (stopped || !binding) return
    if ((recordDepth.get(conversationId) ?? 0) > 0) {
      recordDirty.add(conversationId)
      return
    }
    const running = recordPulls.get(conversationId)
    if (running) {
      recordDirty.add(conversationId)
      return running
    }
    const gen = generation
    const run = (async () => {
      recordDepth.set(conversationId, 1)
      try {
        do {
          recordDirty.delete(conversationId)
          await pullRecordOnce(conversationId)
        } while (recordDirty.has(conversationId) && !stopped && generation === gen)
      } finally {
        recordDepth.set(conversationId, 0)
      }
    })()
    recordPulls.set(conversationId, run)
    try {
      await run
    } finally {
      if (recordPulls.get(conversationId) === run) recordPulls.delete(conversationId)
    }
  }

  async function pullRecordOnce(conversationId: string): Promise<void> {
    const watch = recordWatches.get(conversationId)
    if (!watch || !binding || stopped) return
    const gen = generation
    let head: { scope: RecordScope; head: string }
    try {
      headReads += 1
      head = await watch.socket.records.head(
        conversationId,
        binding.receiverId,
        binding.accessEpoch,
      )
    } catch (error) {
      if (stopped || gen !== generation) return
      if (await handleAccess(error)) return
      recordWatches.delete(conversationId)
      return
    }
    if (stopped || gen !== generation || recordWatches.get(conversationId) !== watch)
      return
    const previous = watch.scope
    const moved = headMoved(watch.head, head.head)
    if (previous && !sameScope(previous, head.scope)) {
      watch.scope = head.scope
      watch.head = head.head
      savedRecords.set(conversationId, {
        conversationId,
        scope: head.scope,
        head: head.head,
      })
      await readView(conversationId, gen)
      return
    }
    watch.scope = head.scope
    if (moved === "same") return
    watch.head = head.head
    savedRecords.set(conversationId, {
      conversationId,
      scope: head.scope,
      head: head.head,
    })
    if (moved === "rewound") {
      catalogue = undefined
      await pullCatalogue(true)
    }
    await readView(conversationId, gen)
  }

  async function watchRecord(
    conversationId: string,
    socket: CommitSocket,
    extra: boolean,
  ): Promise<boolean> {
    if (!binding) return false
    try {
      const result = await socket.watches.records({
        conversationId,
        receiverId: binding.receiverId,
        accessEpoch: binding.accessEpoch,
      })
      const saved = savedRecords.get(conversationId)
      recordWatches.set(conversationId, {
        conversationId,
        watchId: result.watchId,
        socket,
        extra,
        head: saved?.head,
        scope: saved?.scope,
      })
      return true
    } catch (error) {
      const code = rpcCode(error)
      if (code === "watch_capacity" || code === "watch_duplicate") return false
      if (await handleAccess(error)) return false
      return false
    }
  }

  async function retargetWatches(): Promise<void> {
    if (stopped || !binding) return
    const wanted = [...new Set(options.targets())].slice(0, MAX_RECORD_WATCHES)
    for (const [id, watch] of [...recordWatches]) {
      if (wanted.includes(id)) continue
      recordWatches.delete(id)
      await dropWatch(watch.socket, watch.watchId)
      if (watch.extra) {
        watch.socket.close()
        const at = extras.indexOf(watch.socket)
        if (at !== -1) extras.splice(at, 1)
      }
    }
    let usedMain = [...recordWatches.values()].some((watch) => !watch.extra)
    for (const id of wanted) {
      if (recordWatches.has(id) || stopped) continue
      if (!usedMain) {
        const watched = await watchRecord(id, options.client, false)
        if (watched) {
          usedMain = true
          await pullRecord(id)
        }
        continue
      }
      if (!options.openRecordConnection) continue
      let socket: CommitSocket | undefined
      try {
        socket = await options.openRecordConnection()
      } catch {
        socket = undefined
      }
      if (!socket || stopped) continue
      extras.push(socket)
      listenExtra(socket)
      const watched = await watchRecord(id, socket, true)
      if (!watched) {
        socket.close()
        extras.pop()
        continue
      }
      await pullRecord(id)
    }
  }

  function reregister(): Promise<void> {
    const run = reregistering.then(
      () => reregisterOnce(),
      () => reregisterOnce(),
    )
    reregistering = run.then(
      () => undefined,
      () => undefined,
    )
    return run
  }

  async function reregisterOnce(): Promise<void> {
    await untilReady
    if (stopped || !admitted) return
    generation += 1
    const gen = generation
    const previousCatalogue = catalogueWatchId
    catalogueWatchId = undefined
    const watches = [...recordWatches.values()]
    recordWatches.clear()
    await dropWatch(options.client, previousCatalogue)
    for (const watch of watches) await dropWatch(watch.socket, watch.watchId)
    for (const extra of extras) extra.close()
    extras.length = 0
    if (stopped || generation !== gen) return
    if (!(await readBinding())) return
    if (stopped || generation !== gen) return
    if (!(await registerCatalogue())) return
    if (stopped || generation !== gen) return
    subscribe()
    await retargetWatches()
    if (stopped || generation !== gen) return
    await pullCatalogue(catalogue === undefined)
    if (!stopped && generation === gen) finish("sync")
  }

  return {
    counts,
    checkpoint,
    stop,
    retarget() {
      if (!stopped && admitted) void retargetWatches()
    },
    reregister,
    async start() {
      try {
        subscribe()
        if (!(await readBinding())) return outcome ?? "fallback"
        if (!(await registerCatalogue())) return outcome ?? "fallback"
        await retargetWatches()
        if (stopped) return outcome ?? "fallback"
        await pullCatalogue(catalogue === undefined)
        if (stopped) return outcome ?? "fallback"
        return finish("sync")
      } finally {
        resumeReady()
      }
    },
  }
}
