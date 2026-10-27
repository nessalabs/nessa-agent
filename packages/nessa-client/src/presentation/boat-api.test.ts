import { describe, expect, it, vi } from "vitest"
import { boatStatus, createBoatApi } from "./boat-api.js"

describe("Boat connection", () => {
  it("uses the authenticated connection and sends only the token for setup", async () => {
    const connected = { state: "connected", environment: "nessa", image: "fixture-image" }
    const request = vi.fn(async () => connected)
    const boat = createBoatApi({ request })
    await boat.save({ token: "fixture-token" })
    await boat.status()
    await boat.disconnect()
    expect(request.mock.calls).toEqual([
      ["boat.save", { token: "fixture-token" }],
      ["boat.status", {}],
      ["boat.disconnect", {}],
    ])
  })

  it("rejects credential-bearing, partial and inherited status projections", () => {
    for (const value of [
      {
        state: "connected",
        token: "must-not-be-returned",
        environment: "nessa",
        image: "fixture-image",
      },
      { state: "connected", environment: "nessa" },
      { state: "locked", image: "fixture-image" },
      Object.create({ state: "disconnected" }),
      { state: "connected", environment: "nessa", image: null },
    ])
      expect(() => boatStatus(value)).toThrow(TypeError)
    expect(boatStatus({ state: "locked" })).toEqual({ state: "locked" })
    expect(boatStatus({ state: "disconnected" })).toEqual({ state: "disconnected" })
  })
})

it("discards untrusted provider diagnostics and secret parameters on failure", async () => {
  const { NessaRpcError } = await import("../application/rpc-error.js")
  const { NessaBoatError } = await import("./boat-api.js")
  const boat = createBoatApi({
    request: async () => {
      throw new NessaRpcError("boat_environment_blocks_credentials", "fixture-secret", {
        token: "fixture-secret",
      })
    },
  })
  const error = await boat
    .save({ token: "fixture-secret" })
    .catch((value: unknown) => value)
  expect(error).toBeInstanceOf(NessaBoatError)
  expect(String(error)).not.toContain("fixture-secret")
  expect(JSON.stringify(error)).not.toContain("fixture-secret")
  expect(error).toMatchObject({ refusal: "boat_environment_blocks_credentials" })
})

it("keeps runtime preferences separate from token setup and channel inheritance", async () => {
  const request = vi.fn(async () => ({
    channelId: "ch_bbbbbbbbbb",
    defaultLifetime: { mode: "after", seconds: 3600 },
    channelOverride: { mode: "never" },
  }))
  const boat = createBoatApi({ request })
  await boat.runtimeSettings({ channelId: "ch_bbbbbbbbbb" })
  await boat.setRuntimeSettings({ channelId: "ch_bbbbbbbbbb", mode: "default" })
  expect(request.mock.calls).toEqual([
    ["boat.runtimeSettings", { channelId: "ch_bbbbbbbbbb" }],
    ["boat.setRuntimeSettings", { channelId: "ch_bbbbbbbbbb", mode: "default" }],
  ])
})
it("rejects contradictory or untrusted lifetime projections", async () => {
  const { boatRuntimeSettings } = await import("./boat-api.js")
  for (const lifetime of [
    { mode: "after" },
    { mode: "after", seconds: 0 },
    { mode: "after", seconds: 4294967296 },
    { mode: "after", seconds: 1.5 },
    { mode: "never", seconds: 3600 },
    Object.create({ mode: "never" }),
  ]) {
    expect(() => boatRuntimeSettings({ defaultLifetime: lifetime })).toThrow(TypeError)
  }
  expect(() =>
    boatRuntimeSettings({ defaultLifetime: { mode: "never" }, token: "untrusted" }),
  ).toThrow(TypeError)
  expect(() =>
    boatRuntimeSettings(Object.create({ defaultLifetime: { mode: "never" } })),
  ).toThrow(TypeError)
  expect(boatRuntimeSettings({ defaultLifetime: { mode: "never" } })).toEqual({
    defaultLifetime: { mode: "never" },
  })
})

it("rejects a settings response for another scope and inherited input scope before effects", async () => {
  const request = vi.fn(async () => ({
    channelId: "ch_cccccccccc",
    defaultLifetime: { mode: "never" },
  }))
  const boat = createBoatApi({ request })
  await expect(boat.runtimeSettings({ channelId: "ch_bbbbbbbbbb" })).rejects.toThrow()
  request.mockClear()
  const inherited = Object.assign(Object.create({ channelId: "ch_bbbbbbbbbb" }), {
    mode: "never",
  })
  await expect(boat.setRuntimeSettings(inherited)).rejects.toThrow()
  expect(request).not.toHaveBeenCalled()
})

it.each(["read", "write"] as const)(
  "keeps an own non-enumerable channel scope in the %s request",
  async (operation) => {
    const request = vi.fn(async (_method: string, params: Record<string, unknown>) => ({
      defaultLifetime: { mode: "never" },
      ...(Object.hasOwn(params, "channelId") ? { channelId: params.channelId } : {}),
    }))
    const boat = createBoatApi({ request })
    const scoped = <T extends object>(params: T) =>
      Object.defineProperty(params, "channelId", { value: "ch_bbbbbbbbbb" })
    const result =
      operation === "read"
        ? await boat.runtimeSettings(scoped({}))
        : await boat.setRuntimeSettings(scoped({ mode: "never" } as const))
    expect(result.channelId).toBe("ch_bbbbbbbbbb")
    expect(request.mock.calls).toEqual([
      [
        operation === "read" ? "boat.runtimeSettings" : "boat.setRuntimeSettings",
        {
          ...(operation === "read" ? {} : { mode: "never" }),
          channelId: "ch_bbbbbbbbbb",
        },
      ],
    ])
  },
)

it.each(["read", "write"] as const)(
  "captures an accessor channel scope once before the %s effect",
  async (operation) => {
    const request = vi.fn(async (_method: string, params: Record<string, unknown>) => ({
      defaultLifetime: { mode: "never" },
      ...(Object.hasOwn(params, "channelId") ? { channelId: params.channelId } : {}),
    }))
    const boat = createBoatApi({ request })
    let reads = 0
    const scoped = <T extends object>(params: T) =>
      Object.defineProperty(params, "channelId", {
        enumerable: true,
        get: () => (++reads === 1 ? "ch_bbbbbbbbbb" : "ch_cccccccccc"),
      })
    const result =
      operation === "read"
        ? await boat.runtimeSettings(scoped({}))
        : await boat.setRuntimeSettings(scoped({ mode: "never" } as const))
    expect(reads).toBe(1)
    expect(result.channelId).toBe("ch_bbbbbbbbbb")
    expect(request.mock.calls[0]?.[1].channelId).toBe("ch_bbbbbbbbbb")
  },
)
