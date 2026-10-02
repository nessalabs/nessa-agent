import { describe, expect, it, vi } from "vitest"
import { createRecordReadApi } from "./record-read-api.js"
import type { RpcRequester } from "../application/session-port.js"
import { bounds, passiveReadTiming, ProductMethod } from "../generated/product.js"
import type { RecordPageRequest, RecordScope } from "../generated/product.js"

const conversationId = "00000000-0000-4000-8000-000000000001"
const scope: RecordScope = {
  receiver: "receiver",
  origin: "gateway",
  stream: conversationId,
  incarnation: "incarnation",
  schema: "nessa.physical-frame.v1",
  accessEpoch: "epoch-3",
}
const request: RecordPageRequest = {
  scope,
  after: "0",
  target: "1",
  maxRecords: bounds.maxRecordPageRecords,
  maxPayloadBytes: bounds.maxPhysicalRecordPayloadBytes,
  maxRecordBytes: bounds.maxPhysicalRecordPayloadBytes,
}

function api(response: unknown) {
  const requester = {
    request: vi.fn().mockResolvedValue(response),
  } satisfies RpcRequester
  return { records: createRecordReadApi(requester), request: requester.request }
}

describe("record read API", () => {
  it("carries the trusted epoch-3 scope unchanged and accepts a bounded head", async () => {
    const { records, request: sent } = api({ scope, head: "1" })
    await expect(records.head(conversationId, "receiver", "3")).resolves.toEqual({
      scope,
      head: "1",
    })
    expect(sent).toHaveBeenCalledWith(
      ProductMethod.ConversationRecordsHead,
      { conversationId, accessEpoch: "3", receiverId: "receiver" },
      { atLeastMs: passiveReadTiming.minRequestTimeoutMs },
    )
  })

  it("decodes a physical record but leaves page validity to sync-engine", async () => {
    const { records } = api({
      request: { ...request, scope: { ...scope } },
      records: [{ position: "1", id: "record", payload: "AQ==" }],
    })
    await expect(records.page(conversationId, "3", request)).resolves.toEqual({
      request,
      records: [{ position: "1", id: "record", payload: new Uint8Array([1]) }],
    })
  })

  it("accepts a maximum physical piece and leaves receiver budget semantics to core", async () => {
    const payload = btoa("x".repeat(bounds.maxPhysicalRecordPayloadBytes))
    const response = {
      request,
      records: [{ position: "1", id: "record", payload }],
    }
    const maximum = api(response)
    const page = await maximum.records.page(conversationId, "3", request)
    expect(page.records[0].payload).toHaveLength(bounds.maxPhysicalRecordPayloadBytes)
    expect(page.records[0].payload[0]).toBe("x".charCodeAt(0))

    const smaller = {
      ...request,
      maxPayloadBytes: bounds.maxPhysicalRecordPayloadBytes - 1,
    }
    const limited = api({ ...response, request: smaller })
    await expect(
      limited.records.page(conversationId, "3", smaller),
    ).resolves.toMatchObject({ request: smaller })
  })

  it("refuses malformed base64 and preserves opaque scope for core validation", async () => {
    const malformed = api({
      request,
      records: [{ position: "1", id: "record", payload: "AR==" }],
    })
    await expect(malformed.records.page(conversationId, "3", request)).rejects.toThrow(
      "Invalid passive read payload",
    )
    const wrong = api({ scope: { ...scope, accessEpoch: "3" }, head: "1" })
    await expect(wrong.records.head(conversationId, "receiver", "3")).resolves.toEqual({
      scope: { ...scope, accessEpoch: "3" },
      head: "1",
    })
  })

  it("preserves page range, echo and opaque IDs for the core semantic owner", async () => {
    const echoed = {
      ...request,
      after: "2",
      target: "1",
      scope: { ...scope, receiver: " " },
    }
    const { records } = api({
      request: echoed,
      records: [{ position: "7", id: "", payload: "AQ==" }],
    })
    await expect(records.page(conversationId, "3", request)).resolves.toEqual({
      request: echoed,
      records: [{ position: "7", id: "", payload: new Uint8Array([1]) }],
    })
  })

  it("refuses invalid bounds and epoch before sending", async () => {
    const { records, request: sent } = api({ request, records: [] })
    await expect(
      records.page(conversationId, "3", { ...request, maxRecords: 64 }),
    ).rejects.toThrow("Invalid record page request")
    await expect(records.head(conversationId, "receiver", "0")).rejects.toThrow(
      "Invalid record scope or binding epoch",
    )
    await expect(
      records.head(conversationId, "receiver", "18446744073709551616"),
    ).rejects.toThrow("Invalid record scope or binding epoch")
    expect(sent).not.toHaveBeenCalled()
  })
})
