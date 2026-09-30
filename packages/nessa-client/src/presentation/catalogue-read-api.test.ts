import { describe, expect, it, vi } from "vitest"
import { catalogueWireSchemas } from "../generated/product.js"
import { createCatalogueReadApi } from "./catalogue-read-api.js"
import type {
  CatalogueManifestRequest,
  CataloguePass,
  ConversationCatalogueResolveParams,
} from "../generated/product.js"

const scope = {
  receiver: "receiver",
  origin: "gateway",
  stream: "owner-stream",
  incarnation: "incarnation",
  schema: "catalogue",
  accessEpoch: "epoch-7",
}
const pass: CataloguePass = {
  scope,
  completed: "0",
  boundary: "18446744073709551615",
  generation: "1",
}
const descriptor = {
  key: { creation: "1", id: "conversation" },
  revision: "2",
  deleted: false,
}
const request: CatalogueManifestRequest = { pass, maxEntries: 32 }
const resolve: ConversationCatalogueResolveParams = {
  pass,
  descriptor,
  maxPayloadBytes: 32,
  accessEpoch: "7",
}

describe("catalogue transport", () => {
  it("discovers current scope and preserves maximum u64 decimal head", async () => {
    const transport = {
      request: vi.fn().mockResolvedValue({ scope, head: "18446744073709551615" }),
    }
    const api = createCatalogueReadApi(transport)
    expect((await api.head({ receiverId: "receiver", accessEpoch: "7" })).head).toBe(
      "18446744073709551615",
    )
    expect(transport.request).toHaveBeenCalledWith("conversation.catalogueHead", {
      receiverId: "receiver",
      accessEpoch: "7",
    })
  })
  it("preserves returned pass evidence for sync-engine correlation", async () => {
    const transport = {
      request: vi
        .fn()
        .mockResolvedValue({ request, entries: [descriptor], hasMore: false }),
    }
    const api = createCatalogueReadApi(transport)
    expect((await api.manifest({ request, accessEpoch: "7" })).entries).toEqual([
      descriptor,
    ])
    transport.request.mockResolvedValue({
      request: { ...request, pass: { ...pass, generation: "2" } },
      entries: [descriptor],
      hasMore: false,
    })
    expect(
      (await api.manifest({ request, accessEpoch: "7" })).request.pass.generation,
    ).toBe("2")
    // Shape-valid but semantically invalid pass remains the engine's decision.
    const semanticRequest = {
      pass: { ...pass, boundary: "0", generation: "0" },
      maxEntries: 32,
    }
    transport.request.mockResolvedValue({
      request: semanticRequest,
      entries: [],
      hasMore: false,
    })
    expect(await api.manifest({ request: semanticRequest, accessEpoch: "7" })).toEqual({
      request: semanticRequest,
      entries: [],
      hasMore: false,
    })
  })
  it("decodes bounded canonical base64 and permits explicit empty deletion payload", async () => {
    const transport = {
      request: vi
        .fn()
        .mockResolvedValue({ pass, descriptor, entry: descriptor, payload: "AQID" }),
    }
    const api = createCatalogueReadApi(transport)
    expect(Array.from((await api.resolve(resolve)).payload)).toEqual([1, 2, 3])
    transport.request.mockResolvedValue({
      pass,
      descriptor,
      entry: { ...descriptor, deleted: true },
      payload: "",
    })
    expect((await api.resolve(resolve)).payload.byteLength).toBe(0)
    transport.request.mockResolvedValue({
      pass,
      descriptor,
      entry: descriptor,
      payload: "AQI=",
    })
    expect(
      (await api.resolve({ ...resolve, maxPayloadBytes: 1 })).payload.byteLength,
    ).toBe(2)
    transport.request.mockResolvedValue({
      pass,
      descriptor,
      entry: descriptor,
      payload: "AR==",
    })
    await expect(api.resolve(resolve)).rejects.toThrow("payload")
  })
  it("refuses generated transport overflow and preserves semantic contradictions for core refusal", async () => {
    const transport = {
      request: vi.fn().mockResolvedValue({
        request,
        entries: Array(
          catalogueWireSchemas.ConversationCatalogueManifestResult.properties.entries
            .maxItems + 1,
        ).fill(descriptor),
        hasMore: true,
      }),
    }
    const api = createCatalogueReadApi(transport)
    await expect(api.manifest({ request, accessEpoch: "7" })).rejects.toThrow(
      "transport shape",
    )
    transport.request.mockResolvedValue({
      request,
      entries: [{ ...descriptor, revision: "0" }],
      hasMore: false,
    })
    expect((await api.manifest({ request, accessEpoch: "7" })).entries[0].revision).toBe(
      "0",
    )
  })
  it("refuses malformed shapes and noncanonical decimals before send", async () => {
    const transport = { request: vi.fn() }
    const api = createCatalogueReadApi(transport)
    for (const boundary of ["01", "-1", "18446744073709551616"]) {
      await expect(
        api.manifest({
          request: { ...request, pass: { ...pass, boundary } },
          accessEpoch: "7",
        }),
      ).rejects.toThrow("transport shape")
    }
    expect(transport.request).not.toHaveBeenCalled()
  })
  it("preserves foreign authority, descriptor, and entry evidence for core refusal", async () => {
    const transport = {
      request: vi
        .fn()
        .mockResolvedValue({ scope: { ...scope, receiver: "other" }, head: "2" }),
    }
    const api = createCatalogueReadApi(transport)
    expect(
      (await api.head({ receiverId: "receiver", accessEpoch: "7" })).scope.receiver,
    ).toBe("other")
    transport.request.mockResolvedValue({
      pass,
      descriptor: { ...descriptor, revision: "3" },
      entry: descriptor,
      payload: "AQ==",
    })
    expect((await api.resolve(resolve)).descriptor.revision).toBe("3")
    transport.request.mockResolvedValue({
      pass,
      descriptor,
      entry: { ...descriptor, key: { ...descriptor.key, id: "foreign" } },
      payload: "AQ==",
    })
    expect((await api.resolve(resolve)).entry.key.id).toBe("foreign")
  })
})
