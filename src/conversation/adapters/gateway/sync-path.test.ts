import {
  NessaRpcError,
  type CatalogueDescriptor,
  type ConversationView,
  type RecordScope,
} from "@nessa/client"
import { expect, it } from "vitest"

import {
  createCommitFollower,
  type CatalogueUpdate,
  type CommitSocket,
  type FollowFallback,
} from "./sync-path"

const scope: RecordScope = {
  receiver: "receiver-1",
  origin: "origin-1",
  stream: "stream-1",
  incarnation: "inc-1",
  schema: "schema-1",
  accessEpoch: "epoch-1",
}

function payload(
  id: string,
  title: string,
  updatedAtMs = 2_000,
  archived = false,
): Uint8Array {
  return new TextEncoder().encode(
    JSON.stringify({
      id,
      createdAtMs: 1_000,
      agent: null,
      model: "claude",
      approvalMode: "ask",
      summary: { title, preview: "Last said", updatedAtMs, archived },
    }),
  )
}

function view(id: string, revision = "r1"): ConversationView {
  return {
    conversationId: id,
    revision,
    approvalMode: "ask",
    approvalModes: [],
    title: "Title",
    queueComplete: true,
    transcriptState: "complete_empty",
    truncated: false,
    messages: [],
    pending: [],
    permissions: [],
    questions: [],
    tools: [],
    capabilities: {
      queue: true,
      steer: true,
      resume: true,
      permissions: true,
      imageInput: false,
      agentFeatures: {
        permissionDenial: "supported_for_offered_permission_reviews",
        nativeHookSuppression: "unknown",
        compactionReporting: "unsupported_not_implemented",
        modelSwitchReporting: "unsupported_not_implemented",
        permissionDeferral: "unsupported_not_implemented",
        elicitationForwarding: "unknown",
        preToolPolicy: "unknown",
        policyEndTurn: "unknown",
        policyCloseSession: "unknown",
        incomingElicitation: "unknown",
      },
    },
    lifecycle: { phase: "attached" },
  }
}

interface Entry {
  id: string
  revision: string
  deleted?: boolean
  updatedAtMs?: number
  title?: string
  archived?: boolean
  malformed?: boolean
}

class FakeSocket implements CommitSocket {
  binding = { receiverId: "receiver-1", accessEpoch: "1" }
  bindingError: unknown
  head = "1"
  recordScope: RecordScope = scope
  catalogueScope: RecordScope = scope
  catalogueHead = "1"
  entries: Entry[] = [{ id: "chat-a", revision: "1", title: "Alpha" }]
  heads = new Map<string, string>([["chat-a", "1"]])
  views = new Map<string, ConversationView>()
  catalogueWatchId: string | undefined
  recordWatchIds = new Map<string, string>()
  changed: ((payload: { watchId: string }) => void)[] = []
  ended: ((payload: { watchId: string; reason?: string }) => void)[] = []
  connected: ((state: { status: string }) => void)[] = []
  closed = false
  calls = {
    binding: 0,
    head: 0,
    catalogueHead: 0,
    manifest: 0,
    resolve: 0,
    read: 0,
    list: 0,
  }
  bytes = 0
  /** When set, the next catalogue head emits a catalogue ping before it resolves. */
  pingCatalogueDuringHead = false
  /** When set, the next record head for this id emits its ping and then advances. */
  pingRecordDuringHead: string | undefined
  /** How many record-watch registrations should fail with `watch_capacity` first. */
  recordWatchCapacity = 0

  conversation = {
    binding: async () => {
      this.calls.binding += 1
      if (this.bindingError) throw this.bindingError
      this.bytes += JSON.stringify(this.binding).length
      return this.binding
    },
    read: async (id: string) => {
      this.calls.read += 1
      const body = this.views.get(id) ?? view(id)
      this.bytes += JSON.stringify(body).length
      return body
    },
  }

  records = {
    head: async (id: string) => {
      this.calls.head += 1
      if (this.pingRecordDuringHead === id) {
        this.pingRecordDuringHead = undefined
        const watchId = this.recordWatchIds.get(id)
        if (watchId) for (const handler of this.changed) handler({ watchId })
      }
      const head = this.heads.get(id) ?? this.head
      const body = { scope: this.recordScope, head }
      this.bytes += JSON.stringify(body).length
      return body
    },
  }

  catalogue = {
    head: async () => {
      this.calls.catalogueHead += 1
      if (this.pingCatalogueDuringHead && this.catalogueWatchId) {
        this.pingCatalogueDuringHead = false
        const watchId = this.catalogueWatchId
        for (const handler of this.changed) handler({ watchId })
      }
      const body = { scope: this.catalogueScope, head: this.catalogueHead }
      this.bytes += JSON.stringify(body).length
      return body
    },
    manifest: async () => {
      this.calls.manifest += 1
      const entries = this.entries.map((entry) => ({
        key: { creation: entry.revision, id: entry.id },
        revision: entry.revision,
        deleted: entry.deleted ?? false,
      }))
      const body = {
        request: {
          maxEntries: 256,
          pass: {
            scope: this.catalogueScope,
            completed: "0",
            boundary: this.catalogueHead,
            generation: "1",
          },
        },
        entries,
        hasMore: false,
      }
      this.bytes += JSON.stringify(body).length
      return body
    },
    resolve: async (params: { descriptor: CatalogueDescriptor }) => {
      this.calls.resolve += 1
      const entry = this.entries.find((item) => item.id === params.descriptor.key.id)
      const deleted = entry?.deleted ?? params.descriptor.deleted
      const encoded = entry?.malformed
        ? new TextEncoder().encode("{")
        : payload(
            params.descriptor.key.id,
            entry?.title ?? "Title",
            entry?.updatedAtMs ?? 2_000,
            entry?.archived ?? false,
          )
      this.bytes += encoded.byteLength
      return {
        entry: { ...params.descriptor, deleted },
        payload: deleted ? new Uint8Array() : encoded,
      }
    },
  }

  unwatched: string[] = []

  watches = {
    catalogue: async () => {
      this.catalogueWatchId = "watch-catalogue"
      return { watchId: this.catalogueWatchId }
    },
    records: async (params: { conversationId: string }) => {
      if (this.recordWatchCapacity > 0) {
        this.recordWatchCapacity -= 1
        throw new NessaRpcError("watch_capacity", "watch_capacity")
      }
      const watchId = `watch-${params.conversationId}`
      this.recordWatchIds.set(params.conversationId, watchId)
      return { watchId }
    },
    unwatch: async (watchId: string) => {
      this.unwatched.push(watchId)
      for (const [id, current] of this.recordWatchIds) {
        if (current === watchId) this.recordWatchIds.delete(id)
      }
      if (this.catalogueWatchId === watchId) this.catalogueWatchId = undefined
      return { watchId: "gone" }
    },
  }

  on(
    event: "conversation.changed" | "conversation.watchEnded",
    handler: (payload: { watchId: string; reason?: string }) => void,
  ) {
    const list = event === "conversation.changed" ? this.changed : this.ended
    list.push(handler)
    return () => {
      const at = list.indexOf(handler)
      if (at !== -1) list.splice(at, 1)
    }
  }

  onConnectionStateChange(handler: (state: { status: string }) => void) {
    this.connected.push(handler)
    return () => {
      const at = this.connected.indexOf(handler)
      if (at !== -1) this.connected.splice(at, 1)
    }
  }

  close() {
    this.closed = true
  }

  emitClosed() {
    this.closed = true
    for (const handler of [...this.connected]) handler({ status: "closed" })
  }

  emitCatalogue() {
    if (!this.catalogueWatchId) throw new Error("catalogue watch is not registered")
    for (const handler of [...this.changed]) handler({ watchId: this.catalogueWatchId })
  }

  emitRecord(id: string) {
    const watchId = this.recordWatchIds.get(id)
    if (!watchId) throw new Error(`record watch for ${id} is not registered`)
    for (const handler of [...this.changed]) handler({ watchId })
  }

  endCatalogue() {
    if (!this.catalogueWatchId) throw new Error("catalogue watch is not registered")
    for (const handler of [...this.ended])
      handler({ watchId: this.catalogueWatchId, reason: "closed" })
  }
}

function follow(socket: FakeSocket, targets: () => readonly string[] = () => ["chat-a"]) {
  const catalogues: CatalogueUpdate[] = []
  const views: string[] = []
  const fallbacks: FollowFallback[] = []
  const follower = createCommitFollower({
    client: socket,
    targets,
    onCatalogue: (update) => catalogues.push(update),
    onView: (id) => views.push(id),
    onFallback: (reason) => fallbacks.push(reason),
  })
  return { follower, catalogues, views, fallbacks }
}

const flush = async () => {
  for (let i = 0; i < 40; i++) await Promise.resolve()
}

it("rechecks after the watch is registered and skips a view when the head is unchanged", async () => {
  const socket = new FakeSocket()
  const followed = follow(socket)
  expect(await followed.follower.start()).toBe("sync")
  expect(socket.catalogueWatchId).toBe("watch-catalogue")
  expect(followed.views).toEqual(["chat-a"])
  const reads = socket.calls.read
  socket.emitRecord("chat-a")
  await flush()
  expect(socket.calls.read).toBe(reads)
  expect(socket.calls.head).toBeGreaterThan(1)
})

it("reads the view once when a commit moves the head", async () => {
  const socket = new FakeSocket()
  const followed = follow(socket)
  await followed.follower.start()
  const reads = socket.calls.read
  socket.heads.set("chat-a", "2")
  socket.emitRecord("chat-a")
  await flush()
  expect(socket.calls.read).toBe(reads + 1)
  expect(followed.views).toEqual(["chat-a", "chat-a"])
})

it("coalesces a catalogue ping that arrives during the recheck", async () => {
  const socket = new FakeSocket()
  socket.pingCatalogueDuringHead = true
  const followed = follow(socket)
  await followed.follower.start()
  expect(socket.calls.catalogueHead).toBe(2)
  const heads = socket.calls.catalogueHead
  await flush()
  expect(socket.calls.catalogueHead).toBe(heads)
  expect(followed.fallbacks).toEqual([])
})

it("replaces membership when the catalogue scope changes", async () => {
  const socket = new FakeSocket()
  const followed = follow(socket)
  await followed.follower.start()
  socket.catalogueScope = { ...scope, incarnation: "inc-2" }
  socket.catalogueHead = "2"
  socket.entries = [
    { id: "chat-a", revision: "1", title: "Alpha" },
    { id: "chat-b", revision: "2", title: "Beta" },
  ]
  socket.emitCatalogue()
  await flush()
  const last = followed.catalogues.at(-1)
  expect(last?.reset).toBe(true)
  expect(last?.rows.map((row) => row.conversationId).sort()).toEqual(["chat-a", "chat-b"])
})

it("falls back without a membership replace when the catalogue watch ends", async () => {
  const socket = new FakeSocket()
  const followed = follow(socket)
  await followed.follower.start()
  const catalogues = followed.catalogues.length
  socket.endCatalogue()
  await flush()
  expect(followed.fallbacks).toEqual(["watch-ended"])
  expect(followed.catalogues).toHaveLength(catalogues)
  socket.heads.set("chat-a", "9")
  await flush()
  expect(socket.calls.read).toBe(1)
})

it("falls back to the poller when a held binding is refused", async () => {
  const socket = new FakeSocket()
  const followed = follow(socket)
  await followed.follower.start()
  const reads = socket.calls.read
  socket.bindingError = new NessaRpcError("unauthorized", "unauthorized")
  socket.records.head = async () => {
    throw new NessaRpcError("unauthorized", "unauthorized")
  }
  socket.emitRecord("chat-a")
  await flush()
  expect(followed.fallbacks).toEqual(["watch-refused"])
  expect(socket.calls.read).toBe(reads)
  // Stop releases the catalogue watch, so a later ping has no registration.
  expect(socket.catalogueWatchId).toBeUndefined()
  expect(socket.recordWatchIds.size).toBe(0)
})

it("refetches after a later binding is a new epoch", async () => {
  const socket = new FakeSocket()
  const followed = follow(socket)
  await followed.follower.start()
  socket.binding = { receiverId: "receiver-1", accessEpoch: "2" }
  let threw = false
  socket.records.head = async (id: string) => {
    socket.calls.head += 1
    if (!threw) {
      threw = true
      throw new NessaRpcError("stale_epoch", "stale_epoch")
    }
    return { scope: socket.recordScope, head: socket.heads.get(id) ?? "1" }
  }
  socket.emitRecord("chat-a")
  await flush()
  expect(followed.fallbacks).toEqual([])
  expect(followed.catalogues.some((update) => update.reset)).toBe(true)
  const reads = socket.calls.read
  socket.heads.set("chat-a", "4")
  socket.emitRecord("chat-a")
  await flush()
  expect(socket.recordWatchIds.size).toBe(1)
  expect(socket.calls.read).toBe(reads + 1)
})

it("adds a chat created on the catalogue without a view read when it is not open", async () => {
  const socket = new FakeSocket()
  const followed = follow(socket, () => [])
  await followed.follower.start()
  expect(followed.views).toEqual([])
  socket.catalogueHead = "2"
  socket.entries = [
    { id: "chat-a", revision: "1", title: "Alpha" },
    { id: "chat-b", revision: "2", title: "Elsewhere", updatedAtMs: 3_000 },
  ]
  socket.emitCatalogue()
  await flush()
  const added = followed.catalogues.at(-1)
  expect(added?.reset).toBe(false)
  expect(added?.rows.map((row) => row.conversationId)).toContain("chat-b")
  expect(followed.views).toEqual([])
})

it("watches three conversations and reads a fourth from the catalogue row", async () => {
  const socket = new FakeSocket()
  const extras: FakeSocket[] = []
  for (const id of ["chat-b", "chat-c", "chat-d"]) {
    socket.entries.push({ id, revision: "1", title: id })
    socket.heads.set(id, "1")
  }
  const open = ["chat-a", "chat-b", "chat-c", "chat-d"]
  const followed = createCommitFollower({
    client: socket,
    targets: () => open,
    openRecordConnection: async () => {
      const extra = new FakeSocket()
      extra.binding = socket.binding
      extra.heads = socket.heads
      extra.views = socket.views
      extras.push(extra)
      return extra
    },
    onCatalogue: () => {},
    onView: () => {},
    onFallback: () => {},
  })
  await followed.start()
  expect(socket.recordWatchIds.size).toBe(1)
  expect(extras).toHaveLength(2)
  expect(extras.every((extra) => extra.recordWatchIds.size === 1)).toBe(true)
  const reads =
    socket.calls.read + extras.reduce((sum, extra) => sum + extra.calls.read, 0)
  socket.catalogueHead = "3"
  socket.entries = socket.entries.map((entry) =>
    entry.id === "chat-d" ? { ...entry, revision: "3", updatedAtMs: 9_000 } : entry,
  )
  socket.emitCatalogue()
  await flush()
  const after =
    socket.calls.read + extras.reduce((sum, extra) => sum + extra.calls.read, 0)
  expect(after).toBe(reads + 1)
})

it("closes extra record sockets when watches are registered again", async () => {
  const socket = new FakeSocket()
  const extras: FakeSocket[] = []
  socket.entries.push({ id: "chat-b", revision: "1", title: "Beta" })
  socket.heads.set("chat-b", "1")
  const followed = createCommitFollower({
    client: socket,
    targets: () => ["chat-a", "chat-b"],
    openRecordConnection: async () => {
      const extra = new FakeSocket()
      extra.binding = socket.binding
      extra.heads = socket.heads
      extra.views = socket.views
      extras.push(extra)
      return extra
    },
    onCatalogue: () => {},
    onView: () => {},
    onFallback: () => {},
  })
  await followed.start()
  expect(extras).toHaveLength(1)
  const first = extras[0]
  if (!first) throw new Error("missing extra socket")
  for (const handler of socket.connected) handler({ status: "connected" })
  await flush()
  expect(first.closed).toBe(true)
  expect(extras.filter((extra) => !extra.closed)).toHaveLength(1)
})

it("falls back when the catalogue payload is malformed and does not invent a row", async () => {
  const socket = new FakeSocket()
  socket.entries = [{ id: "chat-a", revision: "1", malformed: true }]
  const followed = follow(socket, () => [])
  expect(await followed.follower.start()).toBe("fallback")
  expect(followed.fallbacks).toEqual(["malformed"])
  expect(followed.catalogues).toEqual([])
})

it("does not resume after not_bound on the first binding", async () => {
  const socket = new FakeSocket()
  socket.bindingError = new NessaRpcError("not_bound", "not_bound")
  const followed = follow(socket)
  expect(await followed.follower.start()).toBe("fallback")
  expect(followed.fallbacks).toEqual(["unbound"])
  expect(socket.calls.read).toBe(0)
  expect(socket.catalogueWatchId).toBeUndefined()
})

/**
 * Idle cost of one open chat for 60 seconds, estimated from the old poll
 * schedule and these fixtures. It is not a timed run against the gateway.
 * The commit figure is the in-process time from the ping to the view read.
 */
it("estimates idle cost from the poll schedule and times an in-process commit", async () => {
  const idleMs = 60_000
  const listed = {
    conversations: [
      {
        conversationId: "chat-a",
        title: "Alpha",
        preview: "Last said",
        createdAtMs: 1_000,
        updatedAtMs: 2_000,
        running: false,
        archived: false,
      },
    ],
    complete: true,
  }
  const listedBytes = JSON.stringify(listed).length
  const viewBytes = JSON.stringify(view("chat-a")).length
  const before = {
    desktopListReads: idleMs / 1_000,
    desktopListBytes: (idleMs / 1_000) * listedBytes,
    desktopViewReads: 0,
    panelViewReads: idleMs / 2_000,
    panelViewBytes: (idleMs / 2_000) * viewBytes,
    desktopCommitWaitMs: 1_000,
    panelIdleCommitWaitMs: 2_000,
    panelBusyCommitWaitMs: 250,
  }
  const socket = new FakeSocket()
  const followed = follow(socket)
  await followed.follower.start()
  const catchupReads = socket.calls.read
  const catchupBytes = socket.bytes
  expect(socket.calls.list).toBe(0)
  socket.heads.set("chat-a", "2")
  socket.views.set("chat-a", view("chat-a", "r2"))
  const beforeCommit = socket.bytes
  const started = performance.now()
  socket.emitRecord("chat-a")
  await flush()
  const commitWaitMs = performance.now() - started
  const after = {
    idleListReads: 0,
    idleViewReads: 0,
    idleBytes: 0,
    commitWaitMs,
    commitViewReads: socket.calls.read - catchupReads,
    commitBytes: socket.bytes - beforeCommit,
    catchupViewReads: catchupReads,
    catchupBytes,
  }
  expect(after.idleListReads).toBe(0)
  expect(after.commitViewReads).toBe(1)
  expect(Number.isFinite(after.commitWaitMs)).toBe(true)
  expect(after.commitWaitMs).toBeGreaterThanOrEqual(0)
  expect(before.desktopListReads).toBe(60)
  expect(before.panelViewReads).toBe(30)
  expect(after.commitBytes).toBeGreaterThan(0)
  expect(after.commitBytes).toBeLessThan(before.panelViewBytes)
})

it("keeps watches off the command socket", async () => {
  const command = new FakeSocket()
  const watch = new FakeSocket()
  let opened = 0
  const followed = createCommitFollower({
    client: command,
    openWatchConnection: async () => {
      opened += 1
      return watch
    },
    targets: () => ["chat-a"],
    onCatalogue: () => {},
    onView: () => {},
    onFallback: () => {},
  })
  expect(await followed.start()).toBe("sync")
  expect(opened).toBe(1)
  expect(command.catalogueWatchId).toBeUndefined()
  expect(command.recordWatchIds.size).toBe(0)
  expect(watch.catalogueWatchId).toBe("watch-catalogue")
  expect(watch.recordWatchIds.size).toBe(1)
  expect(command.closed).toBe(false)
  watch.emitClosed()
  await flush()
  expect(command.closed).toBe(false)
})

it("retries a record watch refused for capacity", async () => {
  const socket = new FakeSocket()
  socket.recordWatchCapacity = 1
  const followed = follow(socket)
  expect(await followed.follower.start()).toBe("sync")
  expect(socket.recordWatchIds.has("chat-a")).toBe(true)
})

it("ends a record watch when its extra connection closes", async () => {
  const socket = new FakeSocket()
  const extras: FakeSocket[] = []
  socket.entries.push({ id: "chat-b", revision: "1", title: "Beta" })
  socket.heads.set("chat-b", "1")
  const followed = createCommitFollower({
    client: socket,
    targets: () => ["chat-a", "chat-b"],
    openRecordConnection: async () => {
      const extra = new FakeSocket()
      extra.binding = socket.binding
      extra.heads = socket.heads
      extra.views = socket.views
      extras.push(extra)
      return extra
    },
    onCatalogue: () => {},
    onView: () => {},
    onFallback: () => {},
  })
  await followed.start()
  const extra = extras[0]
  if (!extra) throw new Error("missing extra socket")
  expect(extra.recordWatchIds.size).toBe(1)
  extra.emitClosed()
  await flush()
  expect(extra.recordWatchIds.size).toBe(1)
  socket.catalogueHead = "4"
  socket.entries = socket.entries.map((entry) =>
    entry.id === "chat-b" ? { ...entry, revision: "4", updatedAtMs: 8_000 } : entry,
  )
  const reads = socket.calls.read
  socket.emitCatalogue()
  await flush()
  expect(socket.calls.read).toBe(reads + 1)
})

it("seeds five hundred chats from one list and does not resolve each payload", async () => {
  const socket = new FakeSocket()
  socket.entries = Array.from({ length: 500 }, (_, index) => ({
    id: `chat-${index}`,
    revision: "1",
    title: `Chat ${index}`,
  }))
  let lists = 0
  const followed = createCommitFollower({
    client: socket,
    targets: () => [],
    listMembership: async () => {
      lists += 1
      return {
        complete: true,
        rows: socket.entries.map((entry) => ({
          conversationId: entry.id,
          title: entry.title ?? null,
          preview: null,
          updatedAtMs: 1_000,
          createdAtMs: 1_000,
          archived: false,
        })),
      }
    },
    onCatalogue: () => {},
    onView: () => {},
    onFallback: () => {},
  })
  expect(await followed.start()).toBe("sync")
  expect(lists).toBe(1)
  expect(socket.calls.resolve).toBe(0)
  expect(socket.calls.manifest).toBe(0)
})

it("falls back when a second refusal arrives while the epoch is moving", async () => {
  const socket = new FakeSocket()
  const followed = follow(socket)
  await followed.follower.start()
  socket.binding = { receiverId: "receiver-1", accessEpoch: "2" }
  socket.watches.catalogue = async () => {
    throw new NessaRpcError("unauthorized", "unauthorized")
  }
  socket.records.head = async () => {
    socket.calls.head += 1
    throw new NessaRpcError("stale_epoch", "stale_epoch")
  }
  socket.emitRecord("chat-a")
  await flush()
  expect(followed.fallbacks).toEqual(["watch-refused"])
  expect(socket.closed).toBe(false)
})

it("closes extra record sockets when the binding epoch moves", async () => {
  const socket = new FakeSocket()
  const extras: FakeSocket[] = []
  socket.entries.push({ id: "chat-b", revision: "1", title: "Beta" })
  socket.heads.set("chat-b", "1")
  const followed = createCommitFollower({
    client: socket,
    targets: () => ["chat-a", "chat-b"],
    openRecordConnection: async () => {
      const extra = new FakeSocket()
      extra.binding = socket.binding
      extra.heads = socket.heads
      extra.views = socket.views
      extras.push(extra)
      return extra
    },
    onCatalogue: () => {},
    onView: () => {},
    onFallback: () => {},
  })
  await followed.start()
  const first = extras[0]
  if (!first) throw new Error("missing extra socket")
  expect(first.closed).toBe(false)
  expect(first.recordWatchIds.size).toBe(1)
  socket.binding = { receiverId: "receiver-1", accessEpoch: "2" }
  let threw = false
  socket.records.head = async (id: string) => {
    socket.calls.head += 1
    if (!threw) {
      threw = true
      throw new NessaRpcError("stale_epoch", "stale_epoch")
    }
    return { scope: socket.recordScope, head: socket.heads.get(id) ?? "1" }
  }
  socket.emitRecord("chat-a")
  await flush()
  expect(first.closed).toBe(true)
  expect(first.unwatched.length).toBeGreaterThan(0)
  expect(extras.filter((extra) => !extra.closed)).toHaveLength(1)
  expect(socket.closed).toBe(false)
  expect(socket.recordWatchIds.size).toBe(1)
})

it("ends the server watch and closes the extra when a record head fails", async () => {
  const socket = new FakeSocket()
  const extras: FakeSocket[] = []
  const fallbacks: FollowFallback[] = []
  socket.entries.push({ id: "chat-b", revision: "1", title: "Beta" })
  socket.heads.set("chat-b", "1")
  const followed = createCommitFollower({
    client: socket,
    targets: () => ["chat-a", "chat-b"],
    openRecordConnection: async () => {
      const extra = new FakeSocket()
      extra.binding = socket.binding
      extra.heads = socket.heads
      extra.views = socket.views
      extras.push(extra)
      return extra
    },
    onCatalogue: () => {},
    onView: () => {},
    onFallback: (reason) => fallbacks.push(reason),
  })
  await followed.start()
  const extra = extras[0]
  if (!extra) throw new Error("missing extra socket")
  const watchId = extra.recordWatchIds.get("chat-b")
  extra.records.head = async () => {
    extra.calls.head += 1
    throw new NessaRpcError("unavailable", "unavailable")
  }
  extra.emitRecord("chat-b")
  await flush()
  expect(fallbacks).toEqual([])
  expect(extra.closed).toBe(true)
  expect(extra.unwatched).toEqual(watchId ? [watchId] : [])
  expect(extra.recordWatchIds.size).toBe(0)
  expect(socket.closed).toBe(false)
  const reads = socket.calls.read
  socket.catalogueHead = "5"
  socket.entries = socket.entries.map((entry) =>
    entry.id === "chat-b" ? { ...entry, revision: "5", updatedAtMs: 9_000 } : entry,
  )
  socket.emitCatalogue()
  await flush()
  expect(socket.calls.read).toBe(reads + 1)
})
