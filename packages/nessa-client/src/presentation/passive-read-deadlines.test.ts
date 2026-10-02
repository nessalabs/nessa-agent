import { afterEach, describe, expect, it, vi } from "vitest"
import { bounds, passiveReadTiming } from "../generated/product.js"
import { NessaRpcError } from "../application/rpc-error.js"
import { WireSession } from "../transport/wire-session.js"
import { createRecordReadApi } from "./record-read-api.js"
import { createCatalogueReadApi } from "./catalogue-read-api.js"

const conversation = "00000000-0000-4000-8000-000000000001"
const scope = {
  receiver: "receiver",
  origin: "gateway",
  stream: conversation,
  incarnation: "incarnation",
  schema: "nessa.physical-frame.v1",
  accessEpoch: "epoch-3",
}
const recordRequest = {
  scope,
  after: "0",
  target: "1",
  maxRecords: 1,
  maxPayloadBytes: bounds.maxPhysicalRecordPayloadBytes,
  maxRecordBytes: bounds.maxPhysicalRecordPayloadBytes,
}
const catalogueScope = { ...scope, stream: "owner", schema: "catalogue" }
const pass = { scope: catalogueScope, completed: "0", boundary: "1", generation: "1" }
const descriptor = {
  key: { creation: "1", id: conversation },
  revision: "1",
  deleted: false,
}
const manifest = { pass, maxEntries: 1 }

// The socket seam substitutes only byte delivery; the public APIs, correlation,
// timeout ownership, large-response classification and decoders are real.
function connection(requestTimeoutMs?: number) {
  const listeners = new Map<string, (event: { data: string }) => void>()
  const sent: { id: string; method: string }[] = []
  const close = vi.fn()
  const socket = {
    readyState: 1,
    send: (data: string) => sent.push(JSON.parse(data)),
    addEventListener: (name: string, handler: (event: { data: string }) => void) =>
      listeners.set(name, handler),
    close,
  } as unknown as WebSocket
  const session = new WireSession(socket, { requestTimeoutMs })
  return {
    session,
    sent,
    close,
    reply: (index: number, payload: unknown) => {
      const wire = JSON.stringify({ type: "res", id: sent[index].id, ok: true, payload })
      listeners.get("message")!({ data: wire })
      return new TextEncoder().encode(wire).length
    },
    refuse: (index: number) =>
      listeners.get("message")!({
        data: JSON.stringify({
          type: "res",
          id: sent[index].id,
          ok: false,
          error: { code: "read_timeout", message: "Passive read deadline elapsed" },
        }),
      }),
  }
}

afterEach(() => vi.useRealTimers())

describe("passive read response deadlines", () => {
  it("retains record correlation and its large-page allowance past 30 seconds", async () => {
    vi.useFakeTimers()
    const socket = connection()
    const records = createRecordReadApi(socket.session)
    const head = records.head(conversation, "receiver", "3")
    const page = records.page(conversation, "3", recordRequest)
    const failures = vi.fn()
    void head.catch(failures)
    void page.catch(failures)
    await vi.advanceTimersByTimeAsync(35_000)
    socket.reply(0, { scope, head: "1" })
    const bytes = socket.reply(1, {
      request: recordRequest,
      records: [
        {
          position: "1",
          id: "record",
          payload: btoa("x".repeat(bounds.maxPhysicalRecordPayloadBytes)),
        },
      ],
    })
    expect(bytes).toBeGreaterThan(bounds.maxOrdinaryResponseBytes)
    expect(bytes).toBeLessThan(bounds.maxRecordResponseBytes)
    expect(socket.close).not.toHaveBeenCalled()
    expect(failures).not.toHaveBeenCalled()
    await expect(head).resolves.toEqual({ scope, head: "1" })
    expect((await page).records[0].payload).toHaveLength(
      bounds.maxPhysicalRecordPayloadBytes,
    )
    expect(socket.close).not.toHaveBeenCalled()
    const health = socket.session.request("server.health", {})
    socket.reply(2, { uptimeMs: 1 })
    await expect(health).resolves.toEqual({ uptimeMs: 1 })
  })

  it("accepts late catalogue head, manifest and decoded resolve replies", async () => {
    vi.useFakeTimers()
    const socket = connection()
    const catalogue = createCatalogueReadApi(socket.session)
    const head = catalogue.head({ receiverId: "receiver", accessEpoch: "3" })
    const entries = catalogue.manifest({ request: manifest, accessEpoch: "3" })
    const resolved = catalogue.resolve({
      pass,
      descriptor,
      maxPayloadBytes: 3,
      accessEpoch: "3",
    })
    const failures = vi.fn()
    for (const pending of [head, entries, resolved]) void pending.catch(failures)
    await vi.advanceTimersByTimeAsync(35_000)
    expect(failures).not.toHaveBeenCalled()
    socket.reply(0, { scope: catalogueScope, head: "1" })
    socket.reply(1, { request: manifest, entries: [descriptor], hasMore: false })
    socket.reply(2, { pass, descriptor, entry: descriptor, payload: "AQID" })
    await expect(head).resolves.toEqual({ scope: catalogueScope, head: "1" })
    await expect(entries).resolves.toMatchObject({ entries: [descriptor] })
    expect(Array.from((await resolved).payload)).toEqual([1, 2, 3])
    expect(socket.close).not.toHaveBeenCalled()
  })

  it("keeps a longer configured passive deadline and ordinary 30-second deadlines", async () => {
    vi.useFakeTimers()
    const socket = connection(60_000)
    const pending = createRecordReadApi(socket.session).head(
      conversation,
      "receiver",
      "3",
    )
    const failures = vi.fn()
    void pending.catch(failures)
    await vi.advanceTimersByTimeAsync(50_000)
    expect(failures).not.toHaveBeenCalled()
    socket.reply(0, { scope, head: "1" })
    await expect(pending).resolves.toEqual({ scope, head: "1" })
    const ordinary = connection().session.request("server.health", {})
    const failed = expect(ordinary).rejects.toThrow("request timeout: server.health")
    await vi.advanceTimersByTimeAsync(30_001)
    await failed
  })

  it("expires an unanswered passive read at the published floor", async () => {
    vi.useFakeTimers()
    const socket = connection(25)
    const pending = createCatalogueReadApi(socket.session).head({
      receiverId: "receiver",
      accessEpoch: "3",
    })
    const failures = vi.fn()
    void pending.catch(failures)
    await vi.advanceTimersByTimeAsync(passiveReadTiming.minRequestTimeoutMs - 1)
    expect(failures).not.toHaveBeenCalled()
    const failed = expect(pending).rejects.toThrow(
      "request timeout: conversation.catalogueHead",
    )
    await vi.advanceTimersByTimeAsync(1)
    await failed
    expect(failures).toHaveBeenCalledOnce()
    expect(socket.close).not.toHaveBeenCalled()
  })

  it("preserves a delayed typed server read_timeout instead of timing out locally", async () => {
    vi.useFakeTimers()
    const socket = connection()
    const pending = createRecordReadApi(socket.session).head(
      conversation,
      "receiver",
      "3",
    )
    const outcome = pending.catch((error: unknown) => error)
    await vi.advanceTimersByTimeAsync(35_000)
    socket.refuse(0)
    const error = await outcome
    expect(error).toBeInstanceOf(NessaRpcError)
    expect(error).toMatchObject({ code: "read_timeout" })
    expect(socket.close).not.toHaveBeenCalled()
  })
})
